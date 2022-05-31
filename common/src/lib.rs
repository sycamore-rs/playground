use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CompileRequest {
    pub code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CompileResponse {
    Success { js: String, wasm: Vec<u8> },
    CompileError(String),
}
