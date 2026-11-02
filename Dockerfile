# === Stage 1: Builder ===
FROM rust:1.99-slim AS builder

WORKDIR /app

# Install musl development tools on the builder image
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

# Must match server/Cargo.toml.template and flake.nix. Use the musl release
# for the Alpine runtime; update the pinned checksum when changing versions.
RUN set -eux; \
    version=0.2.126; \
    archive="wasm-bindgen-${version}-x86_64-unknown-linux-musl"; \
    curl --fail --location --retry 3 \
        "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/${archive}.tar.gz" \
        --output /tmp/wasm-bindgen.tar.gz; \
    echo '064948d58e2d6c0a745216477a639ba696216d6309aaa902939d1b865b1d869d  /tmp/wasm-bindgen.tar.gz' | sha256sum --check -; \
    tar -xzf /tmp/wasm-bindgen.tar.gz -C /tmp "${archive}/wasm-bindgen"; \
    install -m 0755 "/tmp/${archive}/wasm-bindgen" /usr/local/bin/wasm-bindgen; \
    rm -rf /tmp/wasm-bindgen.tar.gz "/tmp/${archive}"

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
COPY --from=builder /usr/local/bin/wasm-bindgen /usr/local/bin/wasm-bindgen

# Runtime builds are offline. Fetch the full dependency graph: even a WASM
# build may need manifests for target-inactive crates during Cargo resolution.
COPY server/Cargo.toml.template /app/compilation-env/Cargo.toml
RUN mkdir -p compilation-env/src \
    && printf 'fn main() {}\n' > compilation-env/src/main.rs \
    && cargo fetch --manifest-path compilation-env/Cargo.toml

# Build bundled examples and warm the compilation dependencies once per image,
# not on each service startup. The binary creates the compilation workspace.
RUN ./sycamore-playground-server --precompile
EXPOSE 3000
CMD ["./sycamore-playground-server"]
