FROM --platform=amd64 rust:alpine AS builder

# Install build dependencies
RUN apk add --no-cache \
    wget \
    pkgconfig \
    dbus-dev \
    gcc \
    musl-dev \
    build-base

# Download and setup aarch64 cross-compiler
RUN wget https://musl.cc/aarch64-linux-musl-cross.tgz \
    && tar -xf aarch64-linux-musl-cross.tgz \
    && mv aarch64-linux-musl-cross /usr/local/ \
    && rm aarch64-linux-musl-cross.tgz

# Add cross-compiler to PATH
ENV PATH=/usr/local/aarch64-linux-musl-cross/bin:$PATH

# Add Rust targets
RUN rustup target add x86_64-unknown-linux-musl
RUN rustup target add aarch64-unknown-linux-musl

WORKDIR /app

# Build cache layer for backend
COPY ./Cargo.toml .
COPY ./Cargo.lock .

# Create minimal src structure for initial build
RUN mkdir -p ./src/e2e
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/main.rs
RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > ./src/e2e/main.rs

# Build cache layer for unix-utils
RUN mkdir -p ./unix-utils
COPY ./unix-utils/Cargo.toml ./unix-utils/Cargo.toml
COPY ./unix-utils/Cargo.lock ./unix-utils/Cargo.lock
RUN mkdir -p ./unix-utils/src
RUN echo "fn dsa() {}" > ./unix-utils/src/lib.rs

RUN mkdir -p ./hil-processor
COPY ./hil-processor/Cargo.toml ./hil-processor/Cargo.toml
COPY ./hil-processor/Cargo.lock ./hil-processor/Cargo.lock
RUN mkdir -p ./hil-processor/src
RUN echo "fn dsa() {}" > ./hil-processor/src/lib.rs

# Initial builds for dependency caching
RUN cargo build -r --target x86_64-unknown-linux-musl --config target.x86_64-unknown-linux-musl.linker=\"x86_64-alpine-linux-musl-gcc\"
RUN cargo build -r --target aarch64-unknown-linux-musl --config target.aarch64-unknown-linux-musl.linker=\"aarch64-linux-musl-gcc\"

# Clean up artifacts that need to be rebuilt
RUN rm -f target/*/release/deps/unix_utils* target/*/release/deps/libunix_utils* target/*/release/deps/hil_processor* target/*/release/deps/libhil_processor* target/*/release/deps/backend* target/*/release/deps/e2e*

# Copy actual source code and perform final builds
COPY . .
RUN cargo build -r --target x86_64-unknown-linux-musl --config target.x86_64-unknown-linux-musl.linker=\"x86_64-alpine-linux-musl-gcc\"
RUN cargo build -r --target aarch64-unknown-linux-musl --config target.aarch64-unknown-linux-musl.linker=\"aarch64-linux-musl-gcc\"

FROM alpine:latest as backend
WORKDIR /app
COPY --from=builder /app/target/aarch64-unknown-linux-musl/release/backend /app/backend.arm64
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/backend /app/backend.amd64
RUN if [ "$(uname -m)" = "aarch64" ]; then \
    mv /app/backend.arm64 /app/backend; \
    else \
    mv /app/backend.amd64 /app/backend; \
    fi
ENTRYPOINT ["/app/backend"]

FROM alpine:latest as e2e
WORKDIR /app
COPY --from=builder /app/target/aarch64-unknown-linux-musl/release/e2e /app/e2e.arm64
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/e2e /app/e2e.amd64
RUN if [ "$(uname -m)" = "aarch64" ]; then \
    mv /app/e2e.arm64 /app/e2e; \
    else \
    mv /app/e2e.amd64 /app/e2e; \
    fi
ENTRYPOINT ["/app/e2e"]
