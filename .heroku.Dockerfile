FROM rust:1.60

USER root
COPY . /sycamore-playground
WORKDIR /sycamore-playground

# Install trunk
RUN bash -cl "wget -qO- https://github.com/thedodd/trunk/releases/download/v0.14.0/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xzf- && sudo mv ./trunk /usr/bin/"

# Install wasm32-unknown-unknown target
RUN rustup target add wasm32-unknown-unknown

# Build server
RUN cd server && cargo build --release
