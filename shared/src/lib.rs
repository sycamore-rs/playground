//! API types shared by the playground client and server.

use serde::{Deserialize, Serialize};

/// Source code submitted to the server's `/compile` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileRequest {
    pub code: String,
}

/// Successful compilation metadata. Fetch `artifact_url` separately for raw WASM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileResponse {
    pub artifact_url: String,
    /// Duration of the original compilation, including on cache hits.
    pub compilation_time_ms: u64,
    pub warnings: Vec<CompilerDiagnostic>,
    pub cache_hit: bool,
}

/// A warning emitted by rustc, including its human-readable rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub message: String,
    pub rendered: Option<String>,
    pub spans: Vec<DiagnosticSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSpan {
    pub file_name: String,
    pub line_start: u64,
    pub line_end: u64,
    pub column_start: u64,
    pub column_end: u64,
    pub is_primary: bool,
}

/// Initial editor contents. Shared with the server's precompiled example registry.
pub const DEFAULT_CODE: &str = r#"use sycamore::prelude::*;

fn main() {
    sycamore::render(|| {
        view! {
            h1 { "Hello, Sycamore!" }
        }
    });
}
"#;
