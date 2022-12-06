# Downlink Service 

## Testing

1. `cargo run --examples hpr_client` get public key
2. `HPRS=<PUBLIC_KEY> cargo run  --examples downlink-service` Run downlink service
3. `cargo run --examples hpr_client`
4. `cargo run --examples http_client` Run HTTP client