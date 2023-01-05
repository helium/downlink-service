use std::{collections::HashMap, thread, time};

include!("../src/settings.rs");

#[macro_use]
extern crate log;

#[tokio::main]
async fn main() {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "INFO");
    env_logger::init_from_env(env);

    let one_sec = time::Duration::from_millis(1000);
    let client = reqwest::Client::new();
    let mut counter = 1;

    info!("sending fake downlinks every {one_sec:?}");

    let settings = Settings::new(Some("settings.toml".to_string())).unwrap();
    let x = settings.http_listen.find(":").unwrap() + 1;
    let port = &settings.http_listen[x..];
    let url = format!("http://127.0.0.1:{}/api/downlink", port);
    
    info!("connecting to {url}"); 

    loop {
        let mut map = HashMap::new();


        map.insert("payload", "some random payload");
        let s = counter.clone().to_string();
        map.insert("count", &s);

        info!("sending payload {map:?}");

        let res = client
            .post(&url)
            .json(&map)
            .send()
            .await;

        match res {
            Ok(ok) => info!("got OK {}", ok.status()),
            Err(err) => warn!("got ERR {}", err.to_string()),
        }

        counter += 1;
        thread::sleep(one_sec);
    }
}
