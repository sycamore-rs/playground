use std::{
    fmt,
    path::PathBuf,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sycamore_playground_shared::{CompileRequest, CompilerDiagnostic};
use tokio::{io::AsyncReadExt, time::timeout};

/// Durable compilation data, independent of runtime URLs and cache-hit status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompilationMetadata {
    pub compilation_time_ms: u64,
    pub warnings: Vec<CompilerDiagnostic>,
}

pub(crate) struct CompiledArtifact {
    pub wasm: Vec<u8>,
    pub metadata: CompilationMetadata,
}

pub(crate) struct Compiler {
    work_dir: PathBuf,
}

impl Compiler {
    pub async fn initialize(work_dir: PathBuf) -> std::io::Result<Self> {
        tokio::fs::create_dir_all(work_dir.join("src")).await?;
        tokio::fs::write(
            work_dir.join("Cargo.toml"),
            include_str!("../Cargo.toml.template"),
        )
        .await?;
        tokio::fs::write(work_dir.join("src/main.rs"), "fn main() {}").await?;
        Ok(Self { work_dir })
    }

    /// The caller must serialize access to the shared compilation workspace.
    pub async fn compile(
        &self,
        request: &CompileRequest,
        limit: Duration,
    ) -> Result<CompiledArtifact, CompileError> {
        let started = Instant::now();
        tracing::info!(
            source_bytes = request.code.len(),
            timeout_secs = limit.as_secs(),
            "Compilation started"
        );
        tokio::fs::write(self.work_dir.join("src/main.rs"), &request.code)
            .await
            .map_err(CompileError::io)?;
        let output = self.run_cargo(limit).await?;
        let (warnings, diagnostics) = parse_diagnostics(&output.stdout);
        if !output.status.success() {
            tracing::warn!(
                exit_status = %output.status,
                duration_ms = started.elapsed().as_millis() as u64,
                "Compilation failed"
            );
            return Err(CompileError {
                status: StatusCode::BAD_REQUEST,
                message: format!(
                    "Compilation failed:\n{}{}",
                    diagnostics,
                    String::from_utf8_lossy(&output.stderr),
                ),
            });
        }
        let compilation_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let wasm = tokio::fs::read(
            self.work_dir
                .join("target/wasm32-unknown-unknown/debug/sycamore-playground.wasm"),
        )
        .await
        .map_err(CompileError::io)?;
        tracing::info!(
            compilation_time_ms,
            warning_count = warnings.len(),
            wasm_bytes = wasm.len(),
            "Compilation succeeded"
        );
        Ok(CompiledArtifact {
            wasm,
            metadata: CompilationMetadata {
                compilation_time_ms,
                warnings,
            },
        })
    }

    async fn run_cargo(&self, limit: Duration) -> Result<Output, CompileError> {
        let mut command = tokio::process::Command::new("cargo");
        // Kill rustc/linker descendants too before reusing the shared workspace.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .args([
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--message-format=json",
            ])
            .env("CARGO_BUILD_JOBS", "1")
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(CompileError::io)?;

        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let mut stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        #[cfg(unix)]
        let process_group = child.id().unwrap();
        // Bound pipe draining as well as cargo itself.
        let result = timeout(limit, async {
            let status = child.wait().await?;
            let (stdout, stderr) = tokio::join!(&mut stdout_task, &mut stderr_task);
            Ok::<_, std::io::Error>(Output {
                status,
                stdout: stdout.map_err(std::io::Error::other)??,
                stderr: stderr.map_err(std::io::Error::other)??,
            })
        })
        .await;
        let error = match result {
            Ok(Ok(output)) => return Ok(output),
            Ok(Err(error)) => CompileError::io(error),
            Err(_) => {
                tracing::warn!(timeout_secs = limit.as_secs(), "Compilation timed out");
                CompileError {
                    status: StatusCode::REQUEST_TIMEOUT,
                    message: "Compilation timed out".to_owned(),
                }
            }
        };
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(process_group as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = child.kill().await;
        stdout_task.abort();
        stderr_task.abort();
        Err(error)
    }
}

fn parse_diagnostics(stdout: &[u8]) -> (Vec<CompilerDiagnostic>, String) {
    let mut warnings = Vec::new();
    let mut rendered = String::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if event["reason"] != "compiler-message" {
            continue;
        }
        let message = &event["message"];
        if let Some(text) = message["rendered"].as_str() {
            rendered.push_str(text);
        }
        if message["level"] == "warning" {
            match serde_json::from_value(message.clone()) {
                Ok(warning) => warnings.push(warning),
                Err(error) => tracing::warn!(%error, "Failed to parse compiler warning"),
            }
        }
    }
    (warnings, rendered)
}

#[derive(Debug)]
pub(crate) struct CompileError {
    pub status: StatusCode,
    pub message: String,
}

impl CompileError {
    pub fn io(error: std::io::Error) -> Self {
        tracing::error!(%error, "Compilation I/O error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Compilation I/O error".to_owned(),
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CompileError {}
