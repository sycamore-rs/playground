FROM rust:1.60

RUN ls
RUN cd server && cargo build --release
