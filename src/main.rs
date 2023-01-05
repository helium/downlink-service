use axum::{
    body::Bytes, http::StatusCode, response::IntoResponse, routing::post, Extension, Router,
};
use helium_crypto::{PublicKey, Verify};
use helium_proto::{
    services::downlink::{
        http_roaming_server::HttpRoamingServer, HttpRoamingDownlinkV1, HttpRoamingRegisterV1,
    },
    Message,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::broadcast;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use std::time::{SystemTime, UNIX_EPOCH};
use metrics_exporter_prometheus::PrometheusBuilder;
mod settings;

#[macro_use]
extern crate log;

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[derive(Debug, Clone)]
struct State {
    sender: Arc<broadcast::Sender<Bytes>>,
}

#[tokio::main]
async fn main() -> Result {
    // TODO: what do I put here for path?
    let settings = settings::Settings::new(Some("settings.toml".to_string()))?;

    let env = env_logger::Env::default().filter_or("RUST_LOG", &settings.log);
    env_logger::init_from_env(env);

    let metrics_endpoint = String::from(&settings.metrics_listen);
    let metrics_socket: SocketAddr = metrics_endpoint
        .parse()
        .expect("Invalid metrics_endpoint value");

    if let Err(e) = PrometheusBuilder::new()
        .with_http_listener(metrics_socket)
        .install()
    {
        error!("Failed to install Prometheus scrape endpoint: {e}");
    } else {
        info!("Metrics scrape endpoint listening on {metrics_endpoint}");
    }

    let (tx, _rx) = broadcast::channel(128);
    let sender = Arc::new(tx);
    let state = State {
        sender: sender.clone(),
    };

    let http_endpoint = String::from(&settings.http_listen);
    let http_socket: SocketAddr = http_endpoint
        .parse()
        .expect("Invalid http_endpoint value");
    let http_state = state.clone();

    let http_thread = tokio::spawn(async move {
        let app = Router::new()
            .route("/api/downlink", post(downlink_post))
            .layer(Extension(http_state));

        axum::Server::bind(&http_socket)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    info!("HTTP listening on {http_endpoint}");

    let grpc_endpoint = String::from(&settings.grpc_listen);
    let grpc_socket: SocketAddr = grpc_endpoint
        .parse()
        .expect("Invalid grpc_endpoint value");
    let grpc_state = state.clone();

    let grpc_thread = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(HttpRoamingServer::new(grpc_state))
            .serve(grpc_socket)
            .await
            .unwrap();
    });
    info!("GRPC listening on {grpc_endpoint}");

    match &settings.authorized_keys.is_empty() {
        true => warn!("No authorized_keys set"),
        false => info!("Authorized keys {}", &settings.authorized_keys)
    };

    let _ = tokio::try_join!(http_thread, grpc_thread);

    Ok(())
}

async fn downlink_post(state: Extension<State>, body: Bytes) -> impl IntoResponse {
    metrics::increment_counter!("downlink_service_http_downlink_post_hit");

    info!("got donwlink via http {body:?}");
    match state.sender.send(body) {
        Ok(_t) => (StatusCode::ACCEPTED, "Downlink Accepted"),
        Err(_e) => (StatusCode::INTERNAL_SERVER_ERROR, "Downlink Lost"),
    }
}

#[tonic::async_trait]
impl helium_proto::services::downlink::http_roaming_server::HttpRoaming for State {
    type streamStream = ReceiverStream<Result<HttpRoamingDownlinkV1, Status>>;

    async fn stream(
        &self,
        request: Request<HttpRoamingRegisterV1>,
    ) -> Result<tonic::Response<Self::streamStream>, tonic::Status> {
        let mut http_rx = self.sender.subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let roaming_req: HttpRoamingRegisterV1 = request.into_inner();
        let public_key = PublicKey::try_from(roaming_req.signer.clone()).unwrap();
        let b58 = public_key.to_string();

        let authorized: bool = match settings::Settings::new(Some("settings.toml".to_string())) {
            Err(err) => {
                warn!("failed to get authorized_keys {err:?}");
                true
            }
            Ok(setting) => {
                let contained = match &setting.authorized_keys.is_empty() {
                    true => {
                        info!("empty authorized_keys");
                        true
                    }
                    false => setting.authorized_keys.contains(&b58)
                };
                contained
            }
        };

        if authorized {
            match verify_req(roaming_req, public_key) {
                Err(err) => {
                    metrics::increment_counter!("downlink_service_grpc_verify_req_err");
                    warn!("HPR {b58} failed to verify: {err:?}");
                    Err(tonic::Status::unauthenticated(
                        "failed req verification",
                    ))
                }
                Ok(_) => {
                    metrics::increment_gauge!("downlink_service_grpc_connections", 1.0);
                    info!("HPR {b58} verified");
                    info!("HPR {b58} connected");

                    tokio::spawn(async move {
                        while let Ok(body) = http_rx.recv().await {
                            metrics::increment_counter!("downlink_service_grpc_downlink_hit");

                            info!("got donwlink {body:?} sending to {b58:?}");
                            let sending = HttpRoamingDownlinkV1 { data: body.into() };
                            if let Err(_) = tx.send(Ok(sending)).await {
                                break;
                            }
                        }
                        metrics::decrement_gauge!("downlink_service_grpc_connections", 1.0);
                        info!("HPR {b58} disconnected");
                    });

                    Ok(Response::new(ReceiverStream::new(rx)))
                }
            }
        } else {
            metrics::increment_counter!("downlink_service_grpc_unauthorized_req");
            warn!("HPR {b58} unauthorized");
            Err(tonic::Status::permission_denied("unauthorized"))
        }
    }
}

fn verify_req(
    mut req: HttpRoamingRegisterV1,
    public_key: PublicKey,
) -> Result<&'static str, &'static str> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    
    let timestamp = u128::try_from(req.timestamp).unwrap();
    let two_min : u128 = 2*60*1000;

    let result = if timestamp > now - two_min && timestamp < now + two_min {
        let signature = req.signature;
        req.signature = vec![];
        let encoded = &req.encode_to_vec();

        match public_key.verify(encoded, &signature) {
            Err(_err) => Err("Invalid signature"),
            Ok(_) => Ok("ok"),
        }
    } else {
        Err("Invalid timestamp")
    };

    result
}
