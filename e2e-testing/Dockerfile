FROM rust:slim-bookworm AS builder
RUN apt update && apt install pkg-config -y
RUN rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build cache
COPY ./Cargo.toml .
COPY ./Cargo.lock .
RUN mkdir -p ./src
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/main.rs
RUN cargo build --release
RUN rm -f target/release/deps/$(cat Cargo.toml | awk '/name/ {print}' | cut -d '"' -f 2 | sed 's/-/_/')*

COPY . .
RUN cargo build --release
RUN cp -r target/release/$(cat Cargo.toml | awk '/name/ {print}' | cut -d '"' -f 2) /app/e2e

FROM debian:bookworm-slim
WORKDIR /app

COPY --from=builder /app/e2e /app/e2e
ENTRYPOINT ["/app/e2e"]
