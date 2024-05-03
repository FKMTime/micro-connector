FROM rust:slim-bookworm AS builder
RUN apt update && apt install pkg-config libdbus-1-dev -y
RUN rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build cache
#backend
COPY ./Cargo.toml .
COPY ./Cargo.lock .
RUN mkdir -p ./src/e2e
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/main.rs
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/e2e/main.rs

#unix utils
RUN mkdir -p ./unix-utils
COPY ./unix-utils/Cargo.toml ./unix-utils/Cargo.toml
COPY ./unix-utils/Cargo.lock ./unix-utils/Cargo.lock
RUN mkdir -p ./unix-utils/src
RUN echo "fn dsa() {}" > ./unix-utils/src/lib.rs

RUN cargo build --release
RUN rm -f target/release/deps/unix_utils* target/release/deps/libunix_utils* target/release/deps/backend* target/release/deps/e2e*

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim as backend
WORKDIR /app
COPY --from=builder /app/target/release/backend /app/backend
ENTRYPOINT ["/app/backend"]

FROM debian:bookworm-slim as e2e
WORKDIR /app
COPY --from=builder /app/target/release/e2e /app/e2e
ENTRYPOINT ["/app/e2e"]
