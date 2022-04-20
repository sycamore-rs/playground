FROM rust:1.60

RUN cd server && cargo build --release
