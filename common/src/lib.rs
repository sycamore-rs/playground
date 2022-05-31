use std::borrow::Cow;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CompileRequest<'a> {
    pub code: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CompileResponse<'a> {
    Success {
        js: Cow<'a, str>,
        wasm: Cow<'a, [u8]>,
    },
    CompileError(String),
}
