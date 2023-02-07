use anyhow::anyhow;
use axum::{
    body::Bytes, http::StatusCode, response::IntoResponse, routing::post, Extension, Router,
};
use clap::Parser;
use helium_crypto::{PublicKey, Verify};
use helium_proto::{
    services::downlink::{
        http_roaming_server::{self, HttpRoamingServer},
        HttpRoamingDownlinkV1, HttpRoamingRegisterV1,
    },
    Message,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use settings::Settings;
use std::{path::PathBuf, sync::Arc};
use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod settings;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long)]
    config_file: Option<PathBuf>,
}

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[derive(Debug, Clone)]
struct State {
    sender: Arc<broadcast::Sender<Bytes>>,
    settings: Arc<settings::Settings>,
}

impl State {
    fn with_settings(settings: Settings) -> Self {
        let (tx, _rx) = broadcast::channel(128);
        Self {
            sender: Arc::new(tx),
            settings: Arc::new(settings),
        }
    }
}

#[tokio::main]
async fn main() -> Result {
    let cli = Cli::parse();
    let settings = settings::Settings::new(cli.config_file)?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&settings.log))
        .with(tracing_subscriber::fmt::layer())
        .init();

    match &settings.authorized_keys {
        None => warn!("No authorized_keys set"),
        Some(authorized_keys) => info!("Authorized keys {}", authorized_keys),
    };

    if let Err(e) = PrometheusBuilder::new()
        .with_http_listener(settings.metrics_listen)
        .install()
    {
        error!("Failed to install Prometheus scrape endpoint: {e}");
    } else {
        info!(endpoint = %settings.metrics_listen, "Metrics listening");
    }

    let state = State::with_settings(settings);
    let http_state = state.clone();
    let grpc_state = state.clone();

    let http_thread = tokio::spawn(async move {
        let listen = http_state.settings.http_listen;
        let app = Router::new()
            .route("/api/downlink", post(downlink_post))
            .layer(Extension(http_state));

        axum::Server::bind(&listen)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    info!(endpoint = %state.settings.http_listen, "HTTP listening");

    let grpc_thread = tokio::spawn(async move {
        let listen = grpc_state.settings.grpc_listen;
        tonic::transport::Server::builder()
            .add_service(HttpRoamingServer::new(grpc_state))
            .serve(listen)
            .await
            .unwrap();
    });
    info!(endpoint = %state.settings.grpc_listen, "GRPC listening");

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
impl http_roaming_server::HttpRoaming for State {
    type streamStream = ReceiverStream<Result<HttpRoamingDownlinkV1, Status>>;

    async fn stream(
        &self,
        request: Request<HttpRoamingRegisterV1>,
    ) -> Result<tonic::Response<Self::streamStream>, tonic::Status> {
        let mut http_rx = self.sender.subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let roaming_req: HttpRoamingRegisterV1 = request.into_inner();

        match self.settings.authorized_keys.as_ref() {
            None => {
                tokio::spawn(async move {
                    while let Ok(body) = http_rx.recv().await {
                        metrics::increment_counter!("downlink_service_grpc_downlink_hit");

                        info!("got downlink {body:?} sending");
                        let sending = HttpRoamingDownlinkV1 { data: body.into() };
                        if (tx.send(Ok(sending)).await).is_err() {
                            break;
                        }
                    }
                    metrics::decrement_gauge!("downlink_service_grpc_connections", 1.0);
                    info!("disconnected");
                });

                metrics::increment_gauge!("downlink_service_grpc_connections", 1.0);
                info!("verified and connected (no authorized_keys set)");

                Ok(Response::new(ReceiverStream::new(rx)))
            }
            Some(authotized_keys) => match verify_req(roaming_req, authotized_keys.clone()) {
                Err(err) => {
                    metrics::increment_counter!("downlink_service_grpc_verify_req_err");
                    warn!("failed to verify: {err:?}");
                    Err(tonic::Status::unauthenticated("failed req verification"))
                }
                Ok(()) => {
                    tokio::spawn(async move {
                        while let Ok(body) = http_rx.recv().await {
                            metrics::increment_counter!("downlink_service_grpc_downlink_hit");

                            info!("got downlink {body:?} sending");
                            let sending = HttpRoamingDownlinkV1 { data: body.into() };
                            if (tx.send(Ok(sending)).await).is_err() {
                                break;
                            }
                        }
                        metrics::decrement_gauge!("downlink_service_grpc_connections", 1.0);
                        info!("disconnected");
                    });

                    metrics::increment_gauge!("downlink_service_grpc_connections", 1.0);
                    info!("verified and connected");

                    Ok(Response::new(ReceiverStream::new(rx)))
                }
            },
        }
    }
}

fn verify_req(mut req: HttpRoamingRegisterV1, authotized_keys: String) -> Result {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let timestamp = u128::try_from(req.timestamp).unwrap();
    let two_min: u128 = 2 * 60 * 1000;

    if timestamp < (now - two_min) {
        return Err(anyhow!("timestamp too far in the past"));
    }
    if timestamp > (now + two_min) {
        return Err(anyhow!("timestamp too far in the future"));
    }

    let signature = req.signature;
    req.signature = vec![];
    let encoded = &req.encode_to_vec();

    let b58s: Vec<&str> = authotized_keys.split(",").collect();

    for b58 in b58s {
        match PublicKey::from_str(b58) {
            Err(e) => error!("could not parse public key {b58} {e:?}"),
            Ok(public_key) => {
                let result = public_key
                    .verify(encoded, &signature)
                    .map_err(|e| anyhow!("invalid signature: {e:?}"));

                match result {
                    Err(e) => {
                        let b58 = public_key.to_string();
                        error!("verify failed for public key {b58} {e:?}")
                    }
                    Ok(()) => return Ok(()),
                };
            }
        };
    }

    return Err(anyhow!("unauthorized"));
}
