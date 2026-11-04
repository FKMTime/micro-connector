# Builds natively for the platform being built: CI runs each platform on its own runner,
# so there is no cross-compiler and only one Rust target per build
FROM rust:alpine AS builder

# Install build dependencies
RUN apk add --no-cache \
    pkgconfig \
    dbus-dev \
    gcc \
    musl-dev \
    build-base

# Alpine's aarch64 gcc defaults to -moutline-atomics, whose helpers live in libgcc; rustc links
# musl binaries with -nodefaultlibs, so vendored C (libdbus) would fail with undefined __aarch64_* symbols
ENV CFLAGS_aarch64_unknown_linux_musl="-mno-outline-atomics"

WORKDIR /app

# Build dependencies against placeholder sources, so this layer is reused
# until Cargo.toml or Cargo.lock change
COPY ./Cargo.toml ./Cargo.lock ./
COPY ./unix-utils/Cargo.toml ./unix-utils/Cargo.lock ./unix-utils/
RUN mkdir -p ./src/e2e ./unix-utils/src \
    && echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/main.rs \
    && cp ./src/main.rs ./src/e2e/main.rs \
    && touch ./unix-utils/src/lib.rs \
    && cargo build -r --bins --target "$(uname -m)-unknown-linux-musl"

# Copy actual source code. COPY keeps the checkout's mtimes, which can be older than the
# placeholder build, so touch the crate roots to make cargo rebuild the real sources
COPY . .
RUN touch ./src/main.rs ./src/e2e/main.rs ./unix-utils/src/lib.rs

FROM builder AS backend-builder
RUN cargo build -r --bin backend --target "$(uname -m)-unknown-linux-musl" \
    && cp "target/$(uname -m)-unknown-linux-musl/release/backend" /app/backend

FROM builder AS e2e-builder
RUN cargo build -r --bin e2e --target "$(uname -m)-unknown-linux-musl" \
    && cp "target/$(uname -m)-unknown-linux-musl/release/e2e" /app/e2e

FROM alpine:latest AS backend
WORKDIR /app
COPY --from=backend-builder /app/backend /app/backend
ENTRYPOINT ["/app/backend"]

FROM alpine:latest AS e2e
WORKDIR /app
COPY --from=e2e-builder /app/e2e /app/e2e
ENTRYPOINT ["/app/e2e"]
