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

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health_check))
        .route("/compile", post(compile_handler))
        .route("/artifacts/{filename}", get(artifact_handler))
        .with_state(state)
        .layer(middleware::from_fn(log_request));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!(address = %listener.local_addr()?, "HTTP server listening");
    axum::serve(listener, app).await?;
    Ok(())
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
    let Some(key) = filename.strip_suffix(".wasm") else {
        return (StatusCode::NOT_FOUND, [("Cache-Control", "no-store")]).into_response();
    };
    // The cache reads under its lock so eviction cannot remove a file mid-read.
    let result = state.cache.lock().await.download(key).await;
    match result {
        Ok(Some(wasm)) => (
            [
                ("Content-Type", "application/wasm"),
                ("Cache-Control", "private, max-age=300"),
            ],
            wasm,
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
