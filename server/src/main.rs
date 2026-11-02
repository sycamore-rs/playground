use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tokio::{sync::Mutex, time::timeout};

#[tokio::main]
async fn main() {
    let state = AppState::initialize().await;

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health_check))
        .route("/compile", post(compile_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn index() -> &'static str {
    "Sycamore Playground Service v0.1"
}

async fn health_check() -> &'static str {
    "OK"
}

#[derive(Clone)]
struct AppState {
    build_lock: Arc<Mutex<()>>,
    work_dir: Arc<PathBuf>,
}

impl AppState {
    async fn initialize() -> Self {
        let work_dir = Arc::new(std::path::Path::new("./compilation-env/").to_owned());

        tokio::fs::create_dir_all(&*work_dir)
            .await
            .expect("Failed to create work directory");

        let cargo_toml = include_str!("../Cargo.toml.template");

        tokio::fs::write(work_dir.join("Cargo.toml"), cargo_toml)
            .await
            .expect("Failed to write Cargo.toml");

        // Create src directory and main.rs
        let src_dir = work_dir.join("src");
        tokio::fs::create_dir_all(&src_dir)
            .await
            .expect("Failed to create src directory");

        tokio::fs::write(src_dir.join("main.rs"), "fn main() {}")
            .await
            .expect("Failed to write main.rs");

        AppState {
            build_lock: Arc::new(Mutex::new(())),
            work_dir,
        }
    }
}

#[derive(Deserialize)]
struct CompileRequest {
    code: String,
}

async fn compile_handler(
    State(state): State<AppState>,
    Json(payload): Json<CompileRequest>,
) -> Response {
    // Acquire the lock to ensure only one compilation at a time
    let _lock = state.build_lock.lock().await;

    // Write the code to src/main.rs in the temporary directory
    let src_path = state.work_dir.join("src").join("main.rs");
    tokio::fs::write(&src_path, payload.code)
        .await
        .expect("Failed to write main.rs");

    // Run the compilation process using `cargo build` in the temporary directory
    let output = tokio::process::Command::new("cargo")
        .args(["build", "--target", "wasm32-unknown-unknown"])
        .env("CARGO_BUILD_JOBS", "1")
        .current_dir(&*state.work_dir)
        .output();

    // Enforce a 2 second timeout for the compilation process
    let output = match timeout(Duration::from_secs(5), output).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Response::builder()
                .status(500)
                .body(format!("Failed to execute cargo build: {}", e).into())
                .unwrap();
        }
        Err(_) => {
            return Response::builder()
                .status(408)
                .body("Compilation timed out".into())
                .unwrap();
        }
    };

    // Compile error
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return (
            StatusCode::BAD_REQUEST,
            format!("Compilation failed:\n{}", stderr),
        )
            .into_response();
    }

    // Read the compiled wasm file and return it as a response
    let wasm_path = state
        .work_dir
        .join("target/wasm32-unknown-unknown/debug/sycamore-playground.wasm");
    let wasm_bytes = match tokio::fs::read(wasm_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Wasm artifact not found after successful build",
            )
                .into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/wasm")
        .body(axum::body::Body::from(wasm_bytes))
        .unwrap()
}
