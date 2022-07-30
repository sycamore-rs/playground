use std::collections::HashMap;
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::error_handling::HandleErrorLayer;
use axum::extract::{Form, Path};
use axum::handler::Handler;
use axum::http::{Method, StatusCode};
use axum::routing::{get, post};
use axum::{http, BoxError, Json, Router};
use once_cell::sync::Lazy;
use playground_common::{CompileRequest, CompileResponse, PasteRequest};
use serde::Deserialize;
use serde_json::json;
use tokio::{fs, process::Command, sync::Mutex};
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};

const CACHE_DIR: &str = "cache";

async fn get_index() -> &'static str {
    "Sycamore playground compiler service. Source code: https://github.com/sycamore-rs/playground"
}

fn hash_str(s: &str) -> String {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish();
    base64::encode_config(hash.to_le_bytes(), base64::URL_SAFE)
}

/// Compile the code and store the result in a cache. Returns a serialized version of `CompileResponse`.
/// If the code has already been compiled and is found in the cache, returns the cached binary instead of recompiling.
async fn process_compile(CompileRequest { code }: CompileRequest<'_>) -> Result<Vec<u8>> {
    static LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
    static CACHE: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

    let code_hash = hash_str(&code);
    let cache_file_name: PathBuf = [CACHE_DIR, &format!("{code_hash}.bin")].iter().collect();
    // First check if we have a cached version.
    if CACHE.lock().await.contains(&code_hash) {
        // Deserialize the cached file into a `CompileResponse`.
        let res = fs::read(cache_file_name).await?;
        // Return the cached file.
        return Ok(res);
    }

    // Acquire the lock to prevent multiple requests from compiling at the same time.
    let _guard = LOCK.lock().await;

    fs::write("../playground/src/main.rs", code.as_bytes()).await?;

    let cargo_build = Command::new("cargo")
        .env_remove("GITHUB_TOKEN")
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .current_dir("../playground")
        .output()
        .await?;

    if cargo_build.status.success() {
        // Call trunk to orchestrate wasm-bindgen and js glue code generation.
        let _output = Command::new("trunk")
            .env_remove("GITHUB_TOKEN")
            .args(["build", "../playground/index.html", "--filehash", "false"])
            .output()
            .await
            .context("call trunk")?;

        // Read the generated artifacts and serialize them into a `CompileResponse`.
        let wasm = fs::read("../playground/dist/playground_bg.wasm")
            .await
            .context("Could not read wasm artifact.")?;
        let js = fs::read_to_string("../playground/dist/playground.js")
            .await
            .context("Could not read js artifact.")?;
        let res = CompileResponse::Success {
            wasm: wasm.into(),
            js: js.into(),
        };
        let bytes = bincode::serialize(&res).context("Could not serialize result with bincode.")?;

        // Add the generated file to the cache.
        CACHE.lock().await.insert(code_hash);
        fs::create_dir_all(CACHE_DIR).await?;
        fs::write(cache_file_name, &bytes)
            .await
            .context("Could not write cache file.")?;

        Ok(bytes)
    } else {
        // Compile error. We don't want to return `Err(_)` because we want to serialize the error into a `CompileResponse`.
        let res =
            CompileResponse::CompileError(String::from_utf8_lossy(&cargo_build.stderr).to_string());
        let bytes = bincode::serialize(&res)?;
        Ok(bytes)
    }
}

async fn handle_compile(Json(payload): Json<CompileRequest<'_>>) -> (StatusCode, Vec<u8>) {
    match process_compile(payload).await {
        Ok(bytes) => (StatusCode::OK, bytes),
        Err(err) => {
            eprintln!("{err:?}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{err:?}").into_bytes(),
            )
        }
    }
}

async fn handle_timeout_error(err: BoxError) -> (StatusCode, String) {
    if err.is::<tower::timeout::error::Elapsed>() {
        (StatusCode::REQUEST_TIMEOUT, "Request timed out".to_string())
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{err}"))
    }
}

/// Create a GitHub gist and return the id of the new gist.
async fn create_gist(code: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct CreateGistRes {
        id: String,
    }

    let client = reqwest::Client::new();
    let github_token = std::env::var("GITHUB_TOKEN").context("Could not get GITHUB_TOKEN")?;
    let res = client
        .post("https://api.github.com/gists")
        .basic_auth("sycamore-playground", Some(github_token))
        .header("User-Agent", "sycamore-playground")
        .json(&json!({
            "files": {
                "main.rs": { "content": code }
            },
            "public": true
        }))
        .send()
        .await
        .context("sending HTTP request")?;
    let res_text = res.text().await?;
    let gist_id = serde_json::from_str::<CreateGistRes>(&res_text)
        .expect("could not parse github API response")
        .id;
    Ok(gist_id)
}

async fn fetch_gist(id: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct GetGistRes {
        files: HashMap<String, File>,
    }
    #[derive(Deserialize)]
    struct File {
        content: String,
    }

    let res = reqwest::get(&format!("https://api.github.com/gists/{id}")).await?;
    let res_text = res.text().await?;
    let content = serde_json::from_str::<GetGistRes>(&res_text)
        .expect("could not parse github API response")
        .files
        .get("main.rs")
        .expect("missing file main.rs")
        .content
        .clone();
    Ok(content)
}

async fn post_gist(Form(form): Form<PasteRequest<'_>>) -> (StatusCode, String) {
    match create_gist(&form.code).await {
        Ok(paste_url) => (StatusCode::OK, paste_url),
        Err(err) => {
            eprintln!("{err:?}");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}"))
        }
    }
}

async fn get_gist(Path(paste_id): Path<String>) -> (StatusCode, String) {
    match fetch_gist(&paste_id).await {
        Ok(paste_url) => (StatusCode::OK, paste_url),
        Err(err) => {
            eprintln!("{err:?}");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}"))
        }
    }
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(get_index))
        .route(
            "/compile",
            post(
                handle_compile.layer(
                    ServiceBuilder::new()
                        .layer(HandleErrorLayer::new(handle_timeout_error))
                        .timeout(Duration::from_secs(4)),
                ),
            ),
        )
        .route("/paste", post(post_gist))
        .route("/paste/:paste_id", get(get_gist))
        .layer(
            CorsLayer::new()
                .allow_headers(vec![http::header::CONTENT_TYPE])
                .allow_methods(vec![Method::GET, Method::POST, Method::OPTIONS])
                .allow_origin(Any),
        );

    // Run on localhost:PORT.
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    axum::Server::bind(&format!("0.0.0.0:{port}").parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
