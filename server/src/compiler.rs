use std::{
    fmt,
    path::PathBuf,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sycamore_playground_shared::{CompileRequest, CompileResponse, CompilerDiagnostic};
use tokio::{io::AsyncReadExt, time::timeout};

/// Durable compilation data, independent of runtime URLs and cache-hit status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompilationMetadata {
    pub compilation_time_ms: u64,
    pub warnings: Vec<CompilerDiagnostic>,
}

pub(crate) struct CompiledArtifact {
    pub wasm: Vec<u8>,
    pub javascript: Vec<u8>,
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
        let output = self.run_cargo(limit).await.map_err(|mut error| {
            error.response.compilation_time_ms =
                started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            error
        })?;
        let (warnings, errors) = parse_diagnostics(&output.stdout);
        if !output.status.success() {
            tracing::warn!(
                exit_status = %output.status,
                duration_ms = started.elapsed().as_millis() as u64,
                "Compilation failed"
            );
            let mut error = CompileError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "Compilation failed:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            );
            // Cargo can fail without emitting a rustc diagnostic (e.g. dependency
            // resolution failures). Retain stderr as a structured fallback then.
            if !errors.is_empty() {
                error.response.errors = errors;
            }
            error.response.warnings = warnings;
            error.response.compilation_time_ms =
                started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            return Err(error);
        }
        let mut command = tokio::process::Command::new("wasm-bindgen");
        command.args([
            "target/wasm32-unknown-unknown/debug/sycamore-playground.wasm",
            "--target",
            "web",
            "--out-dir",
            "target/bindgen",
            "--out-name",
            "sycamore-playground",
        ]);
        let output = self
            .run_command(command, limit.saturating_sub(started.elapsed()))
            .await
            .map_err(|mut error| {
                error.response.warnings = warnings.clone();
                error.response.compilation_time_ms =
                    started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                error
            })?;
        if !output.status.success() {
            tracing::error!(exit_status = %output.status, "wasm-bindgen failed");
            let mut error = CompileError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "wasm-bindgen failed:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            );
            error.response.warnings = warnings;
            error.response.compilation_time_ms =
                started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            return Err(error);
        }
        let wasm = tokio::fs::read(
            self.work_dir
                .join("target/bindgen/sycamore-playground_bg.wasm"),
        )
        .await
        .map_err(CompileError::io)?;
        let javascript =
            tokio::fs::read(self.work_dir.join("target/bindgen/sycamore-playground.js"))
                .await
                .map_err(CompileError::io)?;
        let compilation_time_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        tracing::info!(
            compilation_time_ms,
            warning_count = warnings.len(),
            wasm_bytes = wasm.len(),
            javascript_bytes = javascript.len(),
            "Compilation succeeded"
        );
        Ok(CompiledArtifact {
            wasm,
            javascript,
            metadata: CompilationMetadata {
                compilation_time_ms,
                warnings,
            },
        })
    }

    async fn run_cargo(&self, limit: Duration) -> Result<Output, CompileError> {
        let mut command = tokio::process::Command::new("cargo");
        command
            .args([
                "build",
                "--offline",
                "--target",
                "wasm32-unknown-unknown",
                "--message-format=json",
            ])
            .env("CARGO_BUILD_JOBS", "1");
        self.run_command(command, limit).await
    }

    async fn run_command(
        &self,
        mut command: tokio::process::Command,
        limit: Duration,
    ) -> Result<Output, CompileError> {
        // Kill tool descendants too before reusing the shared workspace.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
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
        // Bound pipe draining as well as the command itself.
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
                CompileError::new(StatusCode::REQUEST_TIMEOUT, "Compilation timed out")
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

fn parse_diagnostics(stdout: &[u8]) -> (Vec<CompilerDiagnostic>, Vec<CompilerDiagnostic>) {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if event["reason"] != "compiler-message" {
            continue;
        }
        let message = &event["message"];
        let diagnostics = match message["level"].as_str() {
            Some("warning") => &mut warnings,
            Some("error") => &mut errors,
            _ => continue,
        };
        match serde_json::from_value(message.clone()) {
            Ok(diagnostic) => diagnostics.push(diagnostic),
            Err(error) => tracing::warn!(%error, "Failed to parse compiler diagnostic"),
        }
    }
    (warnings, errors)
}

#[derive(Debug)]
pub(crate) struct CompileError {
    pub status: StatusCode,
    pub response: CompileResponse,
}

impl CompileError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            response: CompileResponse {
                artifact_url: None,
                compilation_time_ms: 0,
                warnings: Vec::new(),
                errors: vec![CompilerDiagnostic {
                    message: message.into(),
                    rendered: None,
                    spans: Vec::new(),
                }],
                cache_hit: false,
            },
        }
    }

    pub fn io(error: std::io::Error) -> Self {
        tracing::error!(%error, "Compilation I/O error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Compilation I/O error")
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.response.errors.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            f.write_str(error.rendered.as_deref().unwrap_or(&error.message))?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn command_runner_captures_output_and_failure_status() {
        let directory = tempfile::tempdir().unwrap();
        let compiler = Compiler::initialize(directory.path().to_path_buf())
            .await
            .unwrap();
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "printf output; printf diagnostic >&2; exit 7"]);
        let output = compiler
            .run_command(command, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"output");
        assert_eq!(output.stderr, b"diagnostic");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_runner_enforces_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let compiler = Compiler::initialize(directory.path().to_path_buf())
            .await
            .unwrap();
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "sleep 60"]);
        let error = compiler
            .run_command(command, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn parses_structured_errors_and_warnings() {
        let span = serde_json::json!({
            "file_name": "src/main.rs",
            "line_start": 2, "line_end": 2,
            "column_start": 5, "column_end": 12,
            "is_primary": true
        });
        let event = |level: &str| {
            serde_json::json!({
                "reason": "compiler-message",
                "message": {
                    "level": level,
                    "message": format!("a {level}"),
                    "rendered": format!("rendered {level}\n"),
                    "spans": [span.clone()]
                }
            })
            .to_string()
        };
        let stdout = format!(
            "not json\n{}\n{}\n{}\n{}\n",
            event("warning"),
            event("error"),
            event("note"),
            serde_json::json!({"reason": "build-finished", "success": false})
        );
        let (warnings, errors) = parse_diagnostics(stdout.as_bytes());
        assert_eq!(warnings.len(), 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(warnings[0].message, "a warning");
        assert_eq!(errors[0].message, "a error");
        assert_eq!(errors[0].rendered.as_deref(), Some("rendered error\n"));
        assert_eq!(errors[0].spans[0].line_start, 2);
        assert!(errors[0].spans[0].is_primary);
    }

    #[test]
    fn ignores_unrelated_cargo_output() {
        let (warnings, errors) = parse_diagnostics(b"{\"reason\":\"compiler-artifact\"}\n");
        assert!(warnings.is_empty());
        assert!(errors.is_empty());
    }
}
