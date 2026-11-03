//! API types shared by the playground client and server.

use serde::{Deserialize, Serialize};

/// Source code submitted to the server's `/compile` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileRequest {
    pub code: String,
}

/// Compilation result, for both successful and unsuccessful compilations.
/// On success, fetch `artifact_url` separately for raw WASM; on failure it is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileResponse {
    pub artifact_url: Option<String>,
    /// Duration of the original compilation, including on cache hits.
    /// May be zero for service-level failures.
    pub compilation_time_ms: u64,
    pub warnings: Vec<CompilerDiagnostic>,
    pub errors: Vec<CompilerDiagnostic>,
    pub cache_hit: bool,
}

/// A compiler warning or error, including its human-readable rendering.
/// Server-level errors may have no rendering or source spans.
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

#[component]
fn App() -> View {
    let count = create_signal(0);
    let increment = move |_| {
        count.update(|value| *value += 1);
        console_log!("Count: {}", count.get());
    };

    view! {
        p { "Count: " (count.get()) }
        button(on:click=increment) { "Increment" }
    }
}

fn main() {
    sycamore::render(App);
}
"#;
