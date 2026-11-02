//! API types shared by the playground client and server.

use serde::{Deserialize, Serialize};

/// Source code submitted to the server's `/compile` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileRequest {
    pub code: String,
}
