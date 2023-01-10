use helium_crypto::{KeyTag, KeyType, Keypair, Network, Sign};
use helium_proto::{
    services::downlink::{
        http_roaming_client::HttpRoamingClient, HttpRoamingDownlinkV1, HttpRoamingRegisterV1,
    },
    Message,
};
use rand::rngs::OsRng;
use serde_json::Value;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

include!("../src/settings.rs");

#[macro_use]
extern crate log;

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

fn current_timestamp() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

#[tokio::main]
async fn main() -> Result {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "INFO");
    env_logger::init_from_env(env);

    let generated_keypair: Keypair = Keypair::generate(
        KeyTag {
            network: Network::MainNet,
            key_type: KeyType::Ed25519,
        },
        &mut OsRng,
    );
    let path = "hpr_client_key.bin";
    let keypair: Keypair = match fs::read(path) {
        Err(_e) => generated_keypair,
        Ok(data) => match Keypair::try_from(&data[..]) {
            Err(_e) => generated_keypair,
            Ok(keypair) => keypair,
        },
    };
    fs::write(path, &keypair.to_vec())?;
    let b58 = keypair.public_key().to_string();
    info!("B58 {b58}");

    let settings = Settings::new(Some("settings.toml".to_string()))?;
    let x = settings.grpc_listen.find(":").unwrap() + 1;
    let port = &settings.grpc_listen[x..];
    let url = format!("http://127.0.0.1:{}", port);

    info!("connecting to {url}");

    let mut client = HttpRoamingClient::connect(url).await?;

    let mut request = HttpRoamingRegisterV1 {
        region: 1,
        timestamp: current_timestamp()?,
        signer: keypair.public_key().into(),
        signature: vec![],
    };

    request.signature = request.sign(&keypair)?;
    // request.signature = vec![];

    let mut stream = client.stream(request).await?.into_inner();

    while let Ok(item) = stream.message().await {
        let s: HttpRoamingDownlinkV1 = item.unwrap();
        let data = String::from_utf8_lossy(&s.data);
        let v: Value = serde_json::from_str(&data).unwrap();

        info!("got donwlink {v:#?}");
    }

    Ok(())
}

pub trait MsgSign: Message + std::clone::Clone {
    fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>>
    where
        Self: std::marker::Sized;
}

macro_rules! impl_sign {
    ($txn_type:ty, $( $sig: ident ),+ ) => {
        impl MsgSign for $txn_type {
            fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>> {
                let mut txn = self.clone();
                $(txn.$sig = vec![];)+
                Ok(keypair.sign(&txn.encode_to_vec())?)
            }
        }
    }
}

impl_sign!(HttpRoamingRegisterV1, signature);
