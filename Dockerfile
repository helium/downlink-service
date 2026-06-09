# Multi-stage build for Rust application
FROM rust:bookworm AS base

# Install required dependencies for compilation
RUN apt-get update && apt-get install -y \
    curl \
    cmake \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

FROM base AS builder

# Create app directory
WORKDIR /app

# Copy manifests
COPY rust-toolchain.toml ./rust-toolchain.toml
COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock

# Copy source code (examples/ is required: Cargo.toml declares [[example]] targets)
COPY src ./src
COPY examples ./examples

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim AS runner

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/downlink_service /app/downlink_service

# Default config
COPY settings.toml /app/settings.toml

# Expose http, grpc, and metrics ports
EXPOSE 80 50051 9000

# Set default command
ENTRYPOINT ["/app/downlink_service"]
CMD ["-c", "/app/settings.toml"]
