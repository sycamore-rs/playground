mod cache;
mod compiler;
mod precompiled;

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cache::{ArtifactCache, CacheError};
use compiler::{CompileError, Compiler};
use sycamore_playground_shared::{CompileRequest, CompileResponse, DEFAULT_CODE};
use tokio::sync::{Mutex, Semaphore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
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

        let _build_lock = self.build_lock.lock().await;
        if let Some(response) = self.cache.lock().await.lookup(&key).await {
            return Ok(response);
        }

        let artifact = self
            .compiler
            .compile(&request, Duration::from_secs(5))
            .await?;
        let response = CompileResponse {
            artifact_url: format!("/artifacts/{key}.wasm"),
            compilation_time_ms: artifact.metadata.compilation_time_ms,
            warnings: artifact.metadata.warnings,
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
                CacheError::Capacity => CompileError {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    message: "Artifact cache is full; please retry later".to_owned(),
                },
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
        let mut response = (status, self.message).into_response();
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
            return ([("Cache-Control", "no-store")], Json(metadata)).into_response();
        }
    }
    let Ok(permit) = state.compile_slots.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", "5")],
            "Compile queue is full; please retry later",
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
            eprintln!("Compilation task failed: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Compilation task failed").into_response()
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
