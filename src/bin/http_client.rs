use std::{collections::HashMap, thread, time};

pub type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

#[tokio::main]
async fn main() -> Result {
    let one_sec = time::Duration::from_millis(1000);
    let client = reqwest::Client::new();
    let mut counter = 1;

    println!("sending fake downlinks every {:?}", one_sec);

    while counter < 100 {
        let mut map = HashMap::new();


        map.insert("payload", "some random payload");
        let s = counter.clone().to_string();
        map.insert("count", &s);

        println!("sending payload {:?}", map);

        let res = client
            .post("http://127.0.0.1:3000/api/downlink")
            .json(&map)
            .send()
            .await;

        match res {
            Ok(ok) => println!("got OK {}", ok.status()),
            Err(err) => println!("got ERR {}", err.to_string()),
        }

        counter += 1;
        thread::sleep(one_sec);
    }
    Ok(())
}
