use anyhow::Result;
use axum::{
    routing::{get, post},
    Json, Router,
};
use once_cell::sync::Lazy;
use serde::Deserialize;
use tokio::{fs, process::Command, sync::Mutex};

async fn get_index() -> &'static str {
    "Sycamore playground compiler service. Source code: https://github.com/sycamore-rs/playground"
}

#[derive(Deserialize)]
struct CompileReq {
    code: String,
}

async fn compile(CompileReq { code }: CompileReq) -> Result<String> {
    static LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
    let _guard = LOCK.lock().await;

    fs::write("playground/src/main.rs", code).await?;

    let cargo_build = Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .current_dir("playground")
        .output()
        .await?;

    if cargo_build.status.success() {
        let output = Command::new("trunk")
            .arg("build")
            .arg("playground/index.html")
            .output()
            .await?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "compile error:\n{}",
            String::from_utf8_lossy(&cargo_build.stderr)
        ))
    }
}

async fn post_compile(Json(payload): Json<CompileReq>) -> String {
    match compile(payload).await {
        Ok(out) => out,
        Err(err) => err.to_string(),
    }
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(get_index))
        .route("/compile", post(post_compile));

    // Run on localhost:3000.
    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
