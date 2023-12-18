use anyhow::anyhow;
use axum::{
    body::Bytes, http::StatusCode, response::IntoResponse, routing::get, routing::post, Extension,
    Router,
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
use std::{
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::settings::Settings;

mod settings;

const TWO_MIN: Duration = Duration::from_secs(120);

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long)]
    config_file: Option<PathBuf>,
}

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[derive(Debug, Clone)]
struct State {
    sender: broadcast::Sender<Bytes>,
    authorized_signers: Vec<PublicKey>,
}

impl State {
    fn new(authorized_keys: Vec<PublicKey>) -> Result<Self> {
        let (tx, _rx) = broadcast::channel(128);

        Ok(Self {
            sender: tx,
            authorized_signers: authorized_keys,
        })
    }

    fn verify_req(&self, register: &HttpRoamingRegisterV1) -> Result<Option<String>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let timestamp = Duration::from_millis(register.timestamp);

        if timestamp < (now - TWO_MIN) {
            anyhow::bail!("timestamp too far in the past");
        }

        if timestamp > (now + TWO_MIN) {
            anyhow::bail!("timestamp too far in the future");
        }

        if self.authorized_signers.is_empty() {
            return Ok(None);
        }

        for pubkey in self.authorized_signers.iter() {
            if register.verify(pubkey).is_ok() {
                return Ok(Some(pubkey.to_string()));
            }
        }
        anyhow::bail!("no keys matched")
    }
}

#[tokio::main]
async fn main() -> Result {
    let cli = Cli::parse();
    let settings = Settings::new(cli.config_file)?;

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

    let authorized_keys = parse_authorized_keys(settings.authorized_keys)?;
    let grpc_state = State::new(authorized_keys)?;
    let sender = grpc_state.sender.clone();

    let http_thread = tokio::spawn(async move {
        let app = Router::new()
            .route("/api/downlink", post(downlink_post))
            .route("/health", get(|| async { "ok" }))
            .layer(Extension(sender));

        axum::Server::bind(&settings.http_listen)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    info!(endpoint = %settings.http_listen, "HTTP listening");

    let grpc_thread = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .http2_keepalive_interval(Some(Duration::from_secs(250)))
            .http2_keepalive_timeout(Some(Duration::from_secs(60)))
            .add_service(HttpRoamingServer::new(grpc_state))
            .serve(settings.grpc_listen)
            .await
            .unwrap();
    });
    info!(endpoint = %settings.grpc_listen, "GRPC listening");

    let _ = tokio::try_join!(http_thread, grpc_thread);

    Ok(())
}

fn parse_authorized_keys(keys_str: Option<String>) -> Result<Vec<PublicKey>> {
    let mut authorized_keys = vec![];
    if let Some(authorized_keys_str) = keys_str {
        info!("Authorized keys {authorized_keys_str}");
        for key in authorized_keys_str.split(',') {
            authorized_keys.push(
                PublicKey::from_str(key).map_err(|e| anyhow!("could not parse {key}: {e:?}"))?,
            );
        }
    } else {
        warn!("No authorized_keys set");
    }
    Ok(authorized_keys)
}

async fn downlink_post(
    sender: Extension<broadcast::Sender<Bytes>>,
    body: Bytes,
) -> impl IntoResponse {
    metrics::increment_counter!("downlink_service_http_downlink_post_hit");

    info!("got downlink via http {body:?}");
    match sender.send(body) {
        Ok(_t) => (StatusCode::OK, "Downlink Accepted"),
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
        let roaming_req = request.into_inner();

        let b58 = match self.verify_req(&roaming_req) {
            Ok(None) => {
                info!("no keys, connected");
                "all-b58s".to_string()
            }
            Ok(Some(b58)) => {
                info!(b58, "verified and connected");
                b58
            }
            Err(err) => {
                metrics::increment_counter!("downlink_service_grpc_verify_req_err");
                warn!("failed to verify: {err:?}");
                return Err(tonic::Status::permission_denied("unauthorized"));
            }
        };

        metrics::increment_gauge!("downlink_service_grpc_connections", 1.0);
        let (tx, rx) = tokio::sync::mpsc::channel(20);
        tokio::spawn(async move {
            while let Ok(body) = http_rx.recv().await {
                metrics::increment_counter!("downlink_service_grpc_downlink_hit");

                let sending = HttpRoamingDownlinkV1 { data: body.into() };
                if tx.send(Ok(sending)).await.is_err() {
                    warn!("failed to send to {b58}");
                    break;
                }
            }
            metrics::decrement_gauge!("downlink_service_grpc_connections", 1.0);
            info!(b58, "disconnected");
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

pub trait MsgVerify {
    fn verify(&self, verifier: &PublicKey) -> Result<(), anyhow::Error>;
}

impl MsgVerify for HttpRoamingRegisterV1 {
    fn verify(&self, verifier: &PublicKey) -> Result<(), anyhow::Error> {
        let mut buf = vec![];
        let mut msg = self.clone();
        msg.signature = vec![];
        msg.encode(&mut buf)?;
        verifier
            .verify(&buf, &self.signature)
            .map_err(anyhow::Error::from)
    }
}
