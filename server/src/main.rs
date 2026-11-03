mod cache;
mod compiler;
mod precompiled;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{MatchedPath, Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cache::{ArtifactCache, CacheError};
use compiler::{CompileError, Compiler};
use sycamore_playground_shared::{CompileRequest, CompileResponse, DEFAULT_CODE};
use tokio::sync::{Mutex, Semaphore};
use tower_http::compression::{CompressionLayer, CompressionLevel};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sycamore_playground_server=info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let precompile = match args.as_slice() {
        [] => false,
        [arg] if arg == "--precompile" => true,
        [arg] if arg == "--help" || arg == "-h" => {
            println!(
                "Usage: sycamore-playground-server [--precompile]\n\n\
                 --precompile  Build registered examples into artifact-cache/precompiled and exit.\n\
                 With no arguments, serve HTTP and load any bundled examples.\n\
                 Run from the workspace directory; do not precompile alongside a running server."
            );
            return Ok(());
        }
        _ => return Err("Unknown arguments; use --help for usage".into()),
    };
    tracing::info!(
        mode = if precompile { "precompile" } else { "serve" },
        "Starting server binary"
    );
    let cache_root = PathBuf::from("./artifact-cache");
    let compiler = Compiler::initialize(PathBuf::from("./compilation-env")).await?;
    if precompile {
        return precompiled::build(&cache_root, &compiler).await;
    }
    let state = AppState::initialize(compiler, cache_root).await?;

    let app = app_router(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!(address = %listener.local_addr()?, "HTTP server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health_check))
        .route("/compile", post(compile_handler).options(preflight_handler))
        .route(
            "/artifacts/{filename}",
            get(artifact_handler).layer(
                // A moderate Brotli level balances transfer size and per-download CPU.
                // Only artifact downloads are compressed, not compile or health responses.
                CompressionLayer::new().quality(CompressionLevel::Precise(4)),
            ),
        )
        .with_state(state)
        .layer(middleware::from_fn(cors_headers))
        .layer(middleware::from_fn(log_request))
}

async fn cors_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    headers.insert(
        "Access-Control-Allow-Methods",
        "POST, GET, OPTIONS".parse().unwrap(),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        "Content-Type".parse().unwrap(),
    );
    response
}

async fn preflight_handler() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    // Log the route template, never query strings, source code, or arbitrary URLs.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    if route == "/health" && response.status().is_success() {
        tracing::debug!(%method, %route, status, duration_ms, "HTTP request completed");
    } else {
        tracing::info!(%method, %route, status, duration_ms, "HTTP request completed");
    }
    response
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
    compiler: Arc<Compiler>,
    cache: Arc<Mutex<ArtifactCache>>,
    compile_slots: Arc<Semaphore>,
}

impl AppState {
    async fn initialize(compiler: Compiler, cache_root: PathBuf) -> std::io::Result<Self> {
        // Clear only runtime entries. Bundled files survive restarts unchanged;
        // their request keys are regenerated in the fresh random namespace.
        let runtime_root = cache_root.join("runtime");
        match tokio::fs::remove_dir_all(&runtime_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut cache = ArtifactCache::new(
            &runtime_root,
            env_number("ARTIFACT_CACHE_MAX_BYTES", 512 * 1024 * 1024),
            env_number("ARTIFACT_CACHE_MAX_ENTRIES", 1_000)
                .try_into()
                .expect("ARTIFACT_CACHE_MAX_ENTRIES exceeds platform limits"),
            Duration::from_secs(300),
        )
        .await?;
        precompiled::register(&cache_root, &mut cache).await?;

        Ok(AppState {
            build_lock: Arc::new(Mutex::new(())),
            compiler: Arc::new(compiler),
            cache: Arc::new(Mutex::new(cache)),
            compile_slots: Arc::new(Semaphore::new(8)),
        })
    }

    async fn compile(&self, request: CompileRequest) -> Result<CompileResponse, CompileError> {
        let key = {
            let mut cache = self.cache.lock().await;
            let key = cache.key(&request);
            if let Some(response) = cache.lookup(&key).await {
                return Ok(response);
            }
            key
        };

        tracing::debug!(artifact_id = %key, "Cache miss; waiting for compilation lock");
        let _build_lock = self.build_lock.lock().await;
        if let Some(response) = self.cache.lock().await.lookup(&key).await {
            tracing::debug!(artifact_id = %key, "Compilation cache populated while waiting");
            return Ok(response);
        }

        let artifact = self
            .compiler
            .compile(&request, Duration::from_secs(5))
            .await?;
        let response = CompileResponse {
            artifact_url: Some(format!("/artifacts/{key}.wasm")),
            compilation_time_ms: artifact.metadata.compilation_time_ms,
            warnings: artifact.metadata.warnings,
            errors: Vec::new(),
            cache_hit: false,
        };
        // Publish an independent copy while still holding the compilation lock.
        self.cache
            .lock()
            .await
            .insert(
                key,
                &artifact.wasm,
                &artifact.javascript,
                response.clone(),
                request.code == DEFAULT_CODE,
            )
            .await
            .map_err(|error| match error {
                CacheError::Capacity => {
                    tracing::warn!("Artifact cache capacity exhausted");
                    CompileError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Artifact cache is full; please retry later",
                    )
                }
                CacheError::Io(error) => CompileError::io(error),
            })?;
        Ok(response)
    }
}

fn env_number(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or_else(|| panic!("{name} must be a positive integer")),
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("Invalid {name}: {error}"),
    }
}

impl IntoResponse for CompileError {
    fn into_response(self) -> Response {
        let status = self.status;
        let mut response =
            (status, [("Cache-Control", "no-store")], Json(self.response)).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert("Retry-After", "300".parse().unwrap());
        }
        response
    }
}

async fn compile_handler(
    State(state): State<AppState>,
    Json(payload): Json<CompileRequest>,
) -> Response {
    // Hits bypass queue limits and never wait for the compilation lock.
    {
        let mut cache = state.cache.lock().await;
        let key = cache.key(&payload);
        if let Some(metadata) = cache.lookup(&key).await {
            tracing::debug!(artifact_id = %key, "Compilation cache hit");
            return ([("Cache-Control", "no-store")], Json(metadata)).into_response();
        }
    }
    let Ok(permit) = state.compile_slots.clone().try_acquire_owned() else {
        tracing::warn!("Compile queue is full; rejecting request");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", "5")],
            Json(
                CompileError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Compile queue is full; please retry later",
                )
                .response,
            ),
        )
            .into_response();
    };
    // Finish admitted work if the client disconnects, but bound the running/queued
    // tasks so abandoned requests cannot create an unbounded compilation backlog.
    match tokio::spawn(async move {
        let _permit = permit;
        state.compile(payload).await
    })
    .await
    {
        Ok(Ok(metadata)) => ([("Cache-Control", "no-store")], Json(metadata)).into_response(),
        Ok(Err(error)) => error.into_response(),
        Err(error) => {
            tracing::error!(%error, "Compilation task failed");
            CompileError::new(StatusCode::INTERNAL_SERVER_ERROR, "Compilation task failed")
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_matching_wasm_and_javascript_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let compiler = Compiler::initialize(root.path().join("compilation-env"))
            .await
            .unwrap();
        let mut cache = ArtifactCache::new(
            &root.path().join("cache"),
            4096,
            4,
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        let request = CompileRequest {
            code: "test".into(),
        };
        let key = cache.key(&request);
        let metadata = CompileResponse {
            artifact_url: Some(format!("/artifacts/{key}.wasm")),
            compilation_time_ms: 1,
            warnings: Vec::new(),
            errors: Vec::new(),
            cache_hit: false,
        };
        cache
            .insert(
                key.clone(),
                b"wasm",
                b"export default init;",
                metadata,
                false,
            )
            .await
            .unwrap();
        let state = AppState {
            build_lock: Arc::new(Mutex::new(())),
            compiler: Arc::new(compiler),
            cache: Arc::new(Mutex::new(cache)),
            compile_slots: Arc::new(Semaphore::new(8)),
        };
        for (extension, content_type, body) in [
            ("wasm", "application/wasm", b"wasm".as_slice()),
            ("js", "text/javascript", b"export default init;".as_slice()),
        ] {
            let response =
                artifact_handler(State(state.clone()), Path(format!("{key}.{extension}"))).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], content_type);
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap()
                    .as_ref(),
                body,
            );
        }
        let response = artifact_handler(State(state), Path("missing.js".into())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn artifact_routes_negotiate_brotli_and_preserve_headers() {
        use std::io::Read;
        use tower::ServiceExt;

        let root = tempfile::tempdir().unwrap();
        let compiler = Compiler::initialize(root.path().join("compilation-env"))
            .await
            .unwrap();
        let mut cache = ArtifactCache::new(
            &root.path().join("cache"),
            64 * 1024,
            4,
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        let key = cache.key(&CompileRequest {
            code: "brotli test".into(),
        });
        let wasm = b"\0asm\x01\0\0\0".repeat(256);
        let javascript = b"export const example = 'Brotli compression';\n".repeat(64);
        cache
            .insert(
                key.clone(),
                &wasm,
                &javascript,
                CompileResponse {
                    artifact_url: Some(format!("/artifacts/{key}.wasm")),
                    compilation_time_ms: 1,
                    warnings: Vec::new(),
                    errors: Vec::new(),
                    cache_hit: false,
                },
                false,
            )
            .await
            .unwrap();
        let app = app_router(AppState {
            build_lock: Arc::new(Mutex::new(())),
            compiler: Arc::new(compiler),
            cache: Arc::new(Mutex::new(cache)),
            compile_slots: Arc::new(Semaphore::new(8)),
        });

        for (extension, content_type, original) in [
            ("wasm", "application/wasm", &wasm),
            ("js", "text/javascript", &javascript),
        ] {
            for (accept_encoding, compressed) in [
                (None, false),
                (Some("br"), true),
                (Some("gzip, br"), true),
                (Some("gzip"), false),
                (Some("br;q=0"), false),
            ] {
                let mut request =
                    axum::http::Request::builder().uri(format!("/artifacts/{key}.{extension}"));
                if let Some(encoding) = accept_encoding {
                    request = request.header("Accept-Encoding", encoding);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(axum::body::Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()["content-type"], content_type);
                assert_eq!(response.headers()["cache-control"], "private, max-age=300");
                assert_eq!(response.headers()["access-control-allow-origin"], "*");
                assert!(response.headers().get_all("vary").iter().any(|value| {
                    value
                        .to_str()
                        .unwrap()
                        .split(',')
                        .any(|part| part.trim().eq_ignore_ascii_case("accept-encoding"))
                }));
                if compressed {
                    assert_eq!(response.headers()["content-encoding"], "br");
                    assert!(!response.headers().contains_key("content-length"));
                } else {
                    assert!(!response.headers().contains_key("content-encoding"));
                }
                let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                    .await
                    .unwrap();
                if compressed {
                    assert!(body.len() < original.len());
                    let mut decoded = Vec::new();
                    brotli::Decompressor::new(body.as_ref(), 4096)
                        .read_to_end(&mut decoded)
                        .unwrap();
                    assert_eq!(&decoded, original);
                } else {
                    assert_eq!(body.as_ref(), original.as_slice());
                }
            }
        }
        let health = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .header("Accept-Encoding", "br")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert!(!health.headers().contains_key("content-encoding"));
        let missing = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/artifacts/missing.wasm")
                    .header("Accept-Encoding", "br")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.headers()["cache-control"], "no-store");
    }

    #[tokio::test]
    async fn compile_failures_return_json_with_no_artifact() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let response = CompileError::new(status, "Compilation failed").into_response();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            assert_eq!(response.headers()["cache-control"], "no-store");
            if status == StatusCode::SERVICE_UNAVAILABLE {
                assert_eq!(response.headers()["retry-after"], "300");
            }
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(json["artifact_url"].is_null());
            let result: CompileResponse = serde_json::from_slice(&body).unwrap();
            assert_eq!(result.artifact_url, None);
            assert_eq!(result.errors[0].message, "Compilation failed");
            assert!(result.warnings.is_empty());
            assert!(!result.cache_hit);
        }
    }
}

async fn artifact_handler(State(state): State<AppState>, Path(filename): Path<String>) -> Response {
    let (key, javascript) = if let Some(key) = filename.strip_suffix(".wasm") {
        (key, false)
    } else if let Some(key) = filename.strip_suffix(".js") {
        (key, true)
    } else {
        return (StatusCode::NOT_FOUND, [("Cache-Control", "no-store")]).into_response();
    };
    // The cache reads under its lock so eviction cannot remove a file mid-read.
    let result = if javascript {
        state.cache.lock().await.download_javascript(key).await
    } else {
        state.cache.lock().await.download(key).await
    };
    match result {
        Ok(Some(artifact)) => (
            [
                (
                    "Content-Type",
                    if javascript {
                        "text/javascript"
                    } else {
                        "application/wasm"
                    },
                ),
                ("Cache-Control", "private, max-age=300"),
            ],
            artifact,
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            [("Cache-Control", "no-store")],
            "Artifact expired; resubmit the compile request",
        )
            .into_response(),
        Err(error) => CompileError::io(error).into_response(),
    }
}
