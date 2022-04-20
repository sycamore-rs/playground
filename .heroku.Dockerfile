FROM rust:1.60

USER root
COPY . /sycamore-playground
WORKDIR /sycamore-playground

RUN cd server && cargo build --release
