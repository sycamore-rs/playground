use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use axum::{
    http::{self, Method},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use base64::encode;
use once_cell::sync::Lazy;
use serde::Deserialize;
use tokio::{fs, process::Command, sync::Mutex};
use tower_http::cors::{Any, CorsLayer};

const CACHE_DIR: &str = "cache";

async fn get_index() -> &'static str {
    "Sycamore playground compiler service. Source code: https://github.com/sycamore-rs/playground"
}

#[derive(Deserialize)]
struct CompileReq {
    code: String,
}

fn hash_str(s: &str) -> String {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish();
    base64::encode_config(hash.to_le_bytes(), base64::URL_SAFE)
}

async fn compile(CompileReq { code }: CompileReq) -> Result<Html<String>> {
    static LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    let code_hash = hash_str(&code);
    let cache_file_name: PathBuf = [CACHE_DIR, &format!("{code_hash}.html")].iter().collect();

    let _guard = LOCK.lock().await;

    fs::write("../playground/src/main.rs", code).await?;

    let cargo_build = Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .current_dir("../playground")
        .output()
        .await?;

    if cargo_build.status.success() {
        let _output = Command::new("trunk")
            .arg("build")
            .arg("../playground/index.html")
            .output()
            .await
            .context("call trunk")?;

        pack_into_html(&cache_file_name).await
    } else {
        Err(anyhow::anyhow!(
            "compile error:\n{}",
            String::from_utf8_lossy(&cargo_build.stderr)
        ))
    }
}

async fn pack_into_html(cache_file_name: &Path) -> Result<Html<String>> {
    // wasm file should be in playground/dist
    let mut wasm_files = glob::glob("../playground/dist/*.wasm")
        .context("glob the wasm binary in playground/dist")?;
    let wasm_file = wasm_files
        .next()
        .context("should have exactly 1 wasm file in playground/dist")??;
    let wasm_file_buf = fs::read(wasm_file).await?;
    let wasm_encoded = encode(&wasm_file_buf);
    let mut js_files =
        glob::glob("../playground/dist/*.js").context("glob the js script in playground/dist")?;
    let js_file = js_files
        .next()
        .context("should have exactly 1 js file in playground/dist")??;
    let js_file_buf = fs::read_to_string(js_file).await?;
    let html = format!(
        r#"
    <!DOCTYPE html>
    <html>
        <head>
            <meta content="text/html;charset=utf-8" http-equiv="Content-Type" />

            <script type="module">
                {js_file_buf}

                (async () => {{
                    const data = "data:application/wasm;base64,{wasm_encoded}";
                    await init(data);
                }})();
            </script>
        </head>
        <body>
            <noscript>You need to enable Javascript to run this interactive app.</noscript>
        </body>
    </html>
    "#
    );
    fs::create_dir_all(CACHE_DIR)
        .await
        .context("recursively create cache directory")?;
    fs::write(cache_file_name, &html)
        .await
        .context("writing html file to cache")?;

    Ok(Html(html))
}

async fn post_compile(Json(payload): Json<CompileReq>) -> Html<String> {
    match compile(payload).await {
        Ok(out) => out,
        Err(err) => Html(format!("{:?}", err)),
    }
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(get_index))
        .route("/compile", post(post_compile))
        .layer(
            CorsLayer::new()
                .allow_headers(vec![http::header::CONTENT_TYPE])
                .allow_methods(vec![Method::GET, Method::POST, Method::OPTIONS])
                .allow_origin(Any),
        );

    // Run on localhost:PORT.
    let port = std::env::var("PORT").unwrap_or("3000".to_string());
    axum::Server::bind(&format!("0.0.0.0:{port}").parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
