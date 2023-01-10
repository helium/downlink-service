use std::{collections::HashMap, thread, time};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

include!("../src/settings.rs");

#[tokio::main]
async fn main() {
    let settings = Settings::new(Some("settings.toml".to_string())).unwrap();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&settings.log))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let one_sec = time::Duration::from_millis(1000);
    let client = reqwest::Client::new();
    let mut counter = 1;

    info!("sending fake downlinks every {one_sec:?}");

    let str = settings.http_listen.to_string();
    let x = str.find(":").unwrap() + 1;
    let port = &str[x..];
    let url = format!("http://127.0.0.1:{}/api/downlink", port);

    info!("connecting to {url}");

    loop {
        let mut map = HashMap::new();

        map.insert("payload", "some random payload");
        let s = counter.clone().to_string();
        map.insert("count", &s);

        info!("sending payload {map:?}");

        let res = client.post(&url).json(&map).send().await;

        match res {
            Ok(ok) => info!("got OK {}", ok.status()),
            Err(err) => warn!("got ERR {}", err.to_string()),
        }

        counter += 1;
        thread::sleep(one_sec);
    }
}
