use helium_proto::services::downlink::{
    downlink_client::DownlinkClient, HttpRoamingDownlinkV1, HttpRoamingRegisterV1,
};
use serde_json::Value;

#[macro_use]
extern crate log;

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[tokio::main]
async fn main() -> Result {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "INFO");
    env_logger::init_from_env(env);

    let mut client = DownlinkClient::connect("http://127.0.0.1:50051").await?;

    let request = HttpRoamingRegisterV1 {
        region: 1,
        timestamp: 2,
        signer: vec![],
        signature: vec![],
    };
    let mut stream = client.http_roaming(request).await?.into_inner();

    while let Ok(item) = stream.message().await {
        let s: HttpRoamingDownlinkV1 = item.unwrap();
        let data = String::from_utf8_lossy(&s.data);
        let v: Value = serde_json::from_str(&data).unwrap();

        info!("got donwlink {:#?}", v);
    }

    Ok(())
}
