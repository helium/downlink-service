//! Black-box integration tests for the downlink service.
//!
//! Each test boots the real `downlink_service` binary on ephemeral ports
//! (configured through the `DS__*` environment overrides that `Settings`
//! already understands) and drives it with real HTTP and gRPC clients, in the
//! same way the `examples/http_client.rs` and `examples/hpr_client.rs` clients
//! do.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helium_crypto::{KeyTag, KeyType, Keypair, Network, Sign};
use helium_proto::{
    services::downlink::{http_roaming_client::HttpRoamingClient, HttpRoamingRegisterV1},
    Message,
};
use rand::rngs::OsRng;
use tonic::transport::Channel;

#[tokio::test]
async fn health_endpoint_responds_ok() {
    let server = TestServer::start(None);
    server.wait_until_ready().await;

    let body = reqwest::get(format!("{}/health", server.http_url()))
        .await
        .expect("health request")
        .text()
        .await
        .expect("health body");

    assert_eq!(body, "ok");
}

#[tokio::test]
async fn http_downlink_is_forwarded_to_grpc_stream() {
    let server = TestServer::start(None);
    server.wait_until_ready().await;

    // Register on the gRPC side first so the broadcast subscription is active
    // before we post the downlink.
    let mut client = server.grpc_client().await;
    let register = HttpRoamingRegisterV1 {
        region: 1,
        timestamp: now_ms(),
        signature: vec![],
    };
    let mut stream = client
        .stream(register)
        .await
        .expect("register accepted")
        .into_inner();

    let payload = serde_json::json!({ "payload": "hello", "count": "1" });
    let resp = reqwest::Client::new()
        .post(format!("{}/api/downlink", server.http_url()))
        .json(&payload)
        .send()
        .await
        .expect("post downlink");
    assert!(resp.status().is_success());

    let downlink = tokio::time::timeout(Duration::from_secs(5), stream.message())
        .await
        .expect("timed out waiting for downlink")
        .expect("stream error")
        .expect("stream closed before delivering downlink");

    let received: serde_json::Value =
        serde_json::from_slice(&downlink.data).expect("downlink is json");
    assert_eq!(received, payload);
}

#[tokio::test]
async fn authorized_key_can_register() {
    let keypair = new_keypair();
    let b58 = keypair.public_key().to_string();
    let server = TestServer::start(Some(&b58));
    server.wait_until_ready().await;

    let mut client = server.grpc_client().await;
    let register = signed_register(&keypair, now_ms());

    client
        .stream(register)
        .await
        .expect("authorized register should be accepted");
}

#[tokio::test]
async fn unauthorized_key_is_rejected() {
    let authorized = new_keypair();
    let b58 = authorized.public_key().to_string();
    let server = TestServer::start(Some(&b58));
    server.wait_until_ready().await;

    let mut client = server.grpc_client().await;
    let intruder = new_keypair();
    let register = signed_register(&intruder, now_ms());

    let status = client
        .stream(register)
        .await
        .expect_err("unauthorized register should be rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
}

#[tokio::test]
async fn stale_timestamp_is_rejected() {
    let server = TestServer::start(None);
    server.wait_until_ready().await;

    let mut client = server.grpc_client().await;
    // More than the two-minute window in the past.
    let stale = now_ms() - 5 * 60 * 1000;
    let register = HttpRoamingRegisterV1 {
        region: 1,
        timestamp: stale,
        signature: vec![],
    };

    let status = client
        .stream(register)
        .await
        .expect_err("stale timestamp should be rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
}

/// Ask the OS for a free TCP port on loopback, then release it so the spawned
/// service can bind it. There is a small race window, but it is fine for tests.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_millis() as u64
}

fn new_keypair() -> Keypair {
    Keypair::generate(
        KeyTag {
            network: Network::MainNet,
            key_type: KeyType::Ed25519,
        },
        &mut OsRng,
    )
}

/// Build a register message signed by `keypair`, mirroring the signing scheme
/// used by the service (`signature` is cleared before signing the encoding).
fn signed_register(keypair: &Keypair, timestamp: u64) -> HttpRoamingRegisterV1 {
    let mut register = HttpRoamingRegisterV1 {
        region: 1,
        timestamp,
        signature: vec![],
    };
    let to_sign = register.clone();
    register.signature = keypair
        .sign(&to_sign.encode_to_vec())
        .expect("sign register");
    register
}

/// A running `downlink_service` child process. Killed on drop so a panicking
/// test never leaks a process.
struct TestServer {
    child: Child,
    http_port: u16,
    grpc_port: u16,
}

impl TestServer {
    fn start(authorized_keys: Option<&str>) -> Self {
        let http_port = free_port();
        let grpc_port = free_port();
        let metrics_port = free_port();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_downlink_service"));
        cmd.env("DS__HTTP_LISTEN", format!("127.0.0.1:{http_port}"))
            .env("DS__GRPC_LISTEN", format!("127.0.0.1:{grpc_port}"))
            .env("DS__METRICS_LISTEN", format!("127.0.0.1:{metrics_port}"))
            .env("DS__LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(keys) = authorized_keys {
            cmd.env("DS__AUTHORIZED_KEYS", keys);
        }

        let child = cmd.spawn().expect("spawn downlink_service binary");
        TestServer {
            child,
            http_port,
            grpc_port,
        }
    }

    fn http_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    fn grpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.grpc_port)
    }

    /// Wait for the HTTP `/health` endpoint to answer before driving the test.
    async fn wait_until_ready(&self) {
        let client = reqwest::Client::new();
        let health = format!("{}/health", self.http_url());
        for _ in 0..100 {
            if let Ok(resp) = client.get(&health).send().await {
                if resp.status().is_success() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("downlink_service did not become ready in time");
    }

    /// Connect a gRPC client, retrying until the server is accepting connections.
    async fn grpc_client(&self) -> HttpRoamingClient<Channel> {
        for _ in 0..100 {
            if let Ok(client) = HttpRoamingClient::connect(self.grpc_url()).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("could not connect to gRPC endpoint");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
