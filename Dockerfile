FROM rust:slim-bookworm AS builder
RUN apt update && apt install pkg-config libdbus-1-dev -y
RUN rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build cache
#backend
COPY ./Cargo.toml .
COPY ./Cargo.lock .
RUN mkdir -p ./src
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/main.rs

#unix utils
RUN mkdir -p ./unix-utils
COPY ./unix-utils/Cargo.toml ./unix-utils/Cargo.toml
COPY ./unix-utils/Cargo.lock ./unix-utils/Cargo.lock
RUN mkdir -p ./unix-utils/src
RUN echo "fn dsa() {}" > ./unix-utils/src/lib.rs

RUN cargo build --release
RUN rm -f target/release/deps/unix_utils* target/release/deps/libunix_utils*
RUN rm -f target/release/deps/$(cat Cargo.toml | awk '/name/ {print}' | cut -d '"' -f 2 | sed 's/-/_/')*

#e2e tests
COPY ./e2e-testing/Cargo.toml ./e2e-testing/Cargo.toml
COPY ./e2e-testing/Cargo.lock ./e2e-testing/Cargo.lock
RUN mkdir -p ./e2e-testing/src
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./e2e-testing/src/main.rs

WORKDIR /app/e2e-testing
RUN cargo build --release
RUN rm -f target/release/deps/e2e_testing* target/release/deps/libunix_utils* target/release/deps/unix_utils*

WORKDIR /app
COPY . .
RUN cargo build --release
RUN cp -r target/release/$(cat Cargo.toml | awk '/name/ {print}' | cut -d '"' -f 2) /app/backend

WORKDIR /app/e2e-testing
RUN cargo build --release
RUN cp -r target/release/e2e-testing /app/e2e-testing


FROM debian:bookworm-slim as backend
WORKDIR /app
COPY --from=builder /app/backend /app/backend
ENTRYPOINT ["/app/backend"]

FROM debian:bookworm-slim as e2e
WORKDIR /app
COPY --from=builder /app/e2e-testing /app/e2e-testing
ENTRYPOINT ["/app/e2e-testing"]
