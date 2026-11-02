# === Stage 1: Builder ===
FROM rust:1.99-slim AS builder

WORKDIR /app

# Install musl development tools on the builder image
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

# Copy manifest files first to leverage Docker cache
COPY Cargo.toml Cargo.lock /app/
COPY server/ /app/server/
COPY client/ /app/client/
COPY shared/ /app/shared/

RUN cargo build --locked --release --package sycamore-playground-server --target x86_64-unknown-linux-musl

# === Stage 2: Final Image ===
FROM rust:1.99-alpine

WORKDIR /app

RUN rustup target add wasm32-unknown-unknown

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/sycamore-playground-server /app/sycamore-playground-server

# Build bundled examples and warm the compilation dependencies once per image,
# not on each service startup. The binary creates the compilation workspace.
RUN ./sycamore-playground-server --precompile
EXPOSE 3000
CMD ["./sycamore-playground-server"]
