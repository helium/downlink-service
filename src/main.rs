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
use std::{env, net::SocketAddr, sync::Arc};
use tokio::sync::broadcast;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

#[macro_use]
extern crate log;

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[derive(Debug, Clone)]
struct State {
    sender: Arc<broadcast::Sender<Bytes>>,
}

#[tokio::main]
async fn main() -> Result {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "INFO");

    env_logger::init_from_env(env);

    let (tx, _rx) = broadcast::channel(128);
    let sender = Arc::new(tx);
    let state = State {
        sender: sender.clone(),
    };

    let http_port = match env::var("HTTP_PORT") {
        Ok(val) => val.parse().unwrap(),
        Err(_e) => 3000,
    };

    let http_addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let http_state = state.clone();

    let http_thread = tokio::spawn(async move {
        let app = Router::new()
            .route("/api/downlink", post(downlink_post))
            .layer(Extension(http_state));

        axum::Server::bind(&http_addr)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    info!("HTTP listening on {http_addr}");

    let grpc_port = match env::var("GRPC_PORT") {
        Ok(val) => val.parse().unwrap(),
        Err(_e) => 50051,
    };

    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], grpc_port));
    let grpc_state = state.clone();

    let grpc_thread = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(HttpRoamingServer::new(grpc_state))
            .serve(grpc_addr)
            .await
            .unwrap();
    });

    info!("GRPC listening on {grpc_addr}");

    match env::var("HPRS") {
        Ok(b58s) => info!("Authorized keys {b58s}"),
        Err(_e) => warn!("No keys set via `HPRS=b58,b58`"),
    };

    let _ = tokio::try_join!(http_thread, grpc_thread);

    Ok(())
}

async fn downlink_post(state: Extension<State>, body: Bytes) -> impl IntoResponse {
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

        let authorized = match env::var("HPRS") {
            Ok(b58s) => b58s.contains(&b58),
            Err(_e) => true,
        };

        if authorized {
            match verify_req(roaming_req, public_key) {
                Err(err) => {
                    warn!("HPR {b58} failed to verify: {err:?}");
                    Err(tonic::Status::unauthenticated(
                        "failed singature verification",
                    ))
                }
                Ok(_) => {
                    info!("HPR {b58} verified");
                    info!("HPR {b58} connected");

                    tokio::spawn(async move {
                        while let Ok(body) = http_rx.recv().await {
                            info!("got donwlink {body:?} sending to {b58:?}");
                            let sending = HttpRoamingDownlinkV1 { data: body.into() };
                            if let Err(_) = tx.send(Ok(sending)).await {
                                break;
                            }
                        }
                        info!("HPR {b58} disconnected");
                    });

                    Ok(Response::new(ReceiverStream::new(rx)))
                }
            }
        } else {
            warn!("HPR {b58} unauthorized");
            Err(tonic::Status::permission_denied("unauthorized"))
        }
    }
}

fn verify_req(
    mut req: HttpRoamingRegisterV1,
    public_key: PublicKey,
) -> Result<(), helium_crypto::Error> {
    let signature = req.signature;

    req.signature = vec![];
    let encoded = &req.encode_to_vec();

    public_key.verify(encoded, &signature)
}