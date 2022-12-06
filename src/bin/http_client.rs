use std::{collections::HashMap, thread, time};

#[macro_use]
extern crate log;

#[tokio::main]
async fn main() {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "INFO");
    env_logger::init_from_env(env);

    let one_sec = time::Duration::from_millis(1000);
    let client = reqwest::Client::new();
    let mut counter = 1;

    info!("sending fake downlinks every {:?}", one_sec);

    loop {
        let mut map = HashMap::new();


        map.insert("payload", "some random payload");
        let s = counter.clone().to_string();
        map.insert("count", &s);

        info!("sending payload {:?}", map);

        let res = client
            .post("http://127.0.0.1:3000/api/downlink")
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
