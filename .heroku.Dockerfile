FROM rust:1.63

USER root
COPY . /sycamore-playground
WORKDIR /sycamore-playground

# Install trunk
RUN bash -cl "wget -qO- https://github.com/thedodd/trunk/releases/download/v0.15.0/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xzf- && mv ./trunk /usr/bin/"

# Install wasm32-unknown-unknown target
RUN rustup target add wasm32-unknown-unknown

# Build server
RUN cd server && cargo build --release

# Build playground
RUN cd playground && echo "fn main() {}" > src/main.rs && cargo build --target wasm32-unknown-unknown
