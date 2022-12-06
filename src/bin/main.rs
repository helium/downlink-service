use axum::{
    body::Bytes, http::StatusCode, response::IntoResponse, routing::post, Extension, Router,
};
use helium_proto::services::downlink::{
    downlink_server::DownlinkServer, HttpRoamingDownlinkV1, HttpRoamingRegisterV1,
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

async fn downlink_post(state: Extension<State>, body: Bytes) -> impl IntoResponse {
    info!("got donwlink via http {:?}", body);
    match state.sender.send(body) {
        Ok(_t) => (StatusCode::ACCEPTED, "Downlink Accepted"),
        Err(_e) => (StatusCode::INTERNAL_SERVER_ERROR, "Downlink Lost"),
    }
}

#[tonic::async_trait]
impl helium_proto::services::downlink::downlink_server::Downlink for State {
    type http_roamingStream = ReceiverStream<Result<HttpRoamingDownlinkV1, Status>>;

    async fn http_roaming(
        &self,
        request: Request<HttpRoamingRegisterV1>,
    ) -> Result<tonic::Response<Self::http_roamingStream>, tonic::Status> {
        let mut http_rx = self.sender.subscribe();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let r: HttpRoamingRegisterV1 = request.into_inner();

        info!("HPR connected: {:?}", r);

        tokio::spawn(async move {
            while let Ok(body) = http_rx.recv().await {
                info!("got donwlink {:?} sending to {:?}", body, &r.signer);
                let sending = HttpRoamingDownlinkV1 { data: body.into() };
                if let Err(_) = tx.send(Ok(sending)).await {
                    break;
                }
            }
            info!("HPR disconnected: {:?}", r);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
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
    info!("HTTP listening on {}", http_addr);

    let grpc_port = match env::var("GRPC_PORT") {
        Ok(val) => val.parse().unwrap(),
        Err(_e) => 50051,
    };

    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], grpc_port));
    let grpc_state = state.clone();

    let grpc_thread = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(DownlinkServer::new(grpc_state))
            .serve(grpc_addr)
            .await
            .unwrap();
    });

    info!("GRPC listening on {}", grpc_addr);

    let _ = tokio::try_join!(http_thread, grpc_thread);

    Ok(())
}
