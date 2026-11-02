# AGENTS.md

## Project overview

This repository is the online playground for **sycamore-rs**. It has a client component and a Rust server component. The server is hosted on **Azure Container Apps Express** as a Docker container.

## Repository layout

- `client/` — client-side playground code.
- `server/` — Rust/Axum service. 
- `compilation-env/` — Rust project and build workspace used for compiling submitted playground code to WebAssembly. It is excluded from the root Cargo workspace and not checked into git.
- `Dockerfile` — multi-stage image build for the server and its WebAssembly compilation environment.

## Development and verification

Run Rust commands from the repository root unless a command explicitly says otherwise:

- `cargo check --workspace` — check workspace crates.
- `cargo test --workspace` — run workspace tests.
- `cargo build --release` — build the server in release mode.

The runtime compilation environment needs the `wasm32-unknown-unknown` Rust target. The Docker image sets this up for the container. When changing the container build or compilation environment, verify that the image still builds and that the server can compile a sample program.

## Implementation notes

- Keep server changes in `server/`; its manifest is `server/Cargo.toml`.
- The compile endpoint accepts JSON containing a `code` string and returns a WebAssembly artifact on success. Preserve its response behavior and health endpoint when modifying routes.
- Compilation uses the shared `compilation-env/` workspace and is serialized by a lock. Consider concurrency and cleanup implications when changing this flow.
- Keep the Docker build aligned with the Rust toolchain, server dependencies, and targets needed both to run the service and compile WebAssembly.
- Do not commit generated build output such as `target/` directories or environment-specific secrets.
