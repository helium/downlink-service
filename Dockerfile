FROM rust:1.66

WORKDIR /opt/downlink_service
COPY src/ src/
COPY pkg/ pkg/
COPY settings.toml settings.toml
COPY examples/ examples/
COPY Cargo.lock Cargo.lock
COPY Cargo.toml Cargo.toml

RUN cargo build --release

CMD ["./target/release/downlink_service"]