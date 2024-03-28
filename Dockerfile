FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev pkgconfig dbus-dev

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
RUN cp -r target/release/$(cat Cargo.toml | awk '/name/ {print}' | cut -d '"' -f 2) /app/backend

FROM alpine
WORKDIR /app

COPY --from=builder /app/backend /app/backend

ENTRYPOINT ["/app/backend"]
