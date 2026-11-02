use std::{
    collections::HashMap,
    fmt, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::compiler::CompilationMetadata;
use serde_json::to_vec;
use sha2::{Digest, Sha256};
use sycamore_playground_shared::{CompileRequest, CompileResponse};
use tempfile::{Builder, TempDir};

struct Entry {
    metadata: CompileResponse,
    wasm_path: PathBuf,
    javascript_path: PathBuf,
    size_bytes: u64,
    last_used: Instant,
    protected_until: Instant,
    pinned: bool,
}

/// A process-local artifact cache backed by a randomly namespaced temporary directory.
pub struct ArtifactCache {
    namespace: TempDir,
    entries: HashMap<String, Entry>,
    max_bytes: u64,
    max_entries: usize,
    grace: Duration,
}

#[derive(Debug)]
pub enum CacheError {
    Capacity,
    Io(io::Error),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => write!(f, "artifact cache capacity exceeded"),
            Self::Io(error) => write!(f, "artifact cache I/O error: {error}"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Capacity => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for CacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl ArtifactCache {
    pub async fn new(
        root: &Path,
        max_bytes: u64,
        max_entries: usize,
        grace: Duration,
    ) -> io::Result<Self> {
        tokio::fs::create_dir_all(root).await?;
        let namespace = Builder::new()
            .prefix("playground-artifacts-")
            .rand_bytes(16)
            .tempdir_in(root)?;

        tracing::info!(
            max_bytes,
            max_entries,
            grace_secs = grace.as_secs(),
            "Artifact cache initialized"
        );
        Ok(Self {
            namespace,
            entries: HashMap::new(),
            max_bytes,
            max_entries,
            grace,
        })
    }

    /// Returns a stable key within this process, salted by its random cache namespace.
    pub fn key(&self, request: &CompileRequest) -> String {
        let serialized = to_vec(request).expect("CompileRequest serialization cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(
            self.namespace
                .path()
                .as_os_str()
                .to_string_lossy()
                .as_bytes(),
        );
        hasher.update([0]);
        hasher.update(serialized);
        digest_hex(&hasher.finalize())
    }

    /// Returns cached response metadata, extending the entry's grace period.
    pub async fn lookup(&mut self, key: &str) -> Option<CompileResponse> {
        let now = Instant::now();
        let entry = self.entries.get_mut(key)?;
        entry.last_used = now;
        entry.protected_until = now + self.grace;
        let mut metadata = entry.metadata.clone();
        metadata.cache_hit = true;
        Some(metadata)
    }

    /// Installs an artifact and its response metadata, evicting least-recently-used entries
    /// only when they are neither pinned nor within their grace period.
    pub async fn insert(
        &mut self,
        key: String,
        wasm: &[u8],
        javascript: &[u8],
        mut metadata: CompileResponse,
        pinned: bool,
    ) -> Result<(), CacheError> {
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache keys must be 64 hexadecimal characters",
            )));
        }
        // Compilation is serialized and rechecks the cache before insertion.
        // Never replace an existing entry or temporarily duplicate its disk use.
        if self.entries.contains_key(&key) {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "artifact is already cached",
            )));
        }
        metadata.cache_hit = false;
        let metadata_bytes = to_vec(&metadata)
            .map_err(|error| CacheError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        let entry_size = (wasm.len() as u64)
            .checked_add(javascript.len() as u64)
            .and_then(|size| size.checked_add(metadata_bytes.len() as u64))
            .ok_or(CacheError::Capacity)?;
        if entry_size > self.max_bytes || self.max_entries == 0 {
            return Err(CacheError::Capacity);
        }

        let now = Instant::now();
        let victims = self.plan_evictions(&key, entry_size)?;

        let final_dir = self.entry_dir(&key);
        let stage_dir = self.namespace.path().join(format!(".stage-{key}"));
        if stage_dir.exists() {
            tokio::fs::remove_dir_all(&stage_dir).await?;
        }
        tokio::fs::create_dir(&stage_dir).await?;
        let stage_result = async {
            tokio::fs::write(stage_dir.join("artifact.wasm"), wasm).await?;
            tokio::fs::write(stage_dir.join("artifact.js"), javascript).await?;
            // Keep a diagnostic record beside the WASM. Runtime lookups use the
            // in-memory copy; startup deliberately discards the old namespace.
            tokio::fs::write(stage_dir.join("metadata.json"), &metadata_bytes).await?;
            Ok::<(), io::Error>(())
        }
        .await;
        if let Err(error) = stage_result {
            let _ = tokio::fs::remove_dir_all(&stage_dir).await;
            return Err(CacheError::Io(error));
        }

        if let Err(error) = self.evict_entries(victims).await {
            let _ = tokio::fs::remove_dir_all(&stage_dir).await;
            return Err(CacheError::Io(error));
        }

        if let Err(error) = tokio::fs::rename(&stage_dir, &final_dir).await {
            let _ = tokio::fs::remove_dir_all(&stage_dir).await;
            return Err(CacheError::Io(error));
        }

        tracing::debug!(artifact_id = %key, bytes = entry_size, pinned, "Artifact cached");
        self.entries.insert(
            key,
            Entry {
                metadata,
                wasm_path: final_dir.join("artifact.wasm"),
                javascript_path: final_dir.join("artifact.js"),
                size_bytes: entry_size,
                last_used: now,
                protected_until: Instant::now() + self.grace,
                pinned,
            },
        );
        Ok(())
    }

    /// Registers a bundled artifact without copying it into the runtime cache namespace.
    pub async fn register_precompiled(
        &mut self,
        request: &CompileRequest,
        wasm_path: PathBuf,
        metadata: CompilationMetadata,
    ) -> Result<(), CacheError> {
        let key = self.key(request);
        if self.entries.contains_key(&key) {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "artifact is already cached",
            )));
        }

        let javascript_path = wasm_path.with_file_name("artifact.js");
        let file = tokio::fs::metadata(&wasm_path).await?;
        if !file.is_file() {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "precompiled WASM path is not a file",
            )));
        }
        let wasm_size = file.len();
        let javascript_file = tokio::fs::metadata(&javascript_path).await?;
        if !javascript_file.is_file() {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "precompiled JavaScript path is not a file",
            )));
        }
        let javascript_size = javascript_file.len();
        let metadata_bytes = to_vec(&metadata)
            .map_err(|error| CacheError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        let entry_size = wasm_size
            .checked_add(javascript_size)
            .and_then(|size| size.checked_add(metadata_bytes.len() as u64))
            .ok_or(CacheError::Capacity)?;
        let victims = self.plan_evictions(&key, entry_size)?;
        self.evict_entries(victims).await?;

        let response = CompileResponse {
            artifact_url: Some(format!("/artifacts/{key}.wasm")),
            compilation_time_ms: metadata.compilation_time_ms,
            warnings: metadata.warnings,
            errors: Vec::new(),
            cache_hit: true,
        };
        let now = Instant::now();
        self.entries.insert(
            key,
            Entry {
                metadata: response,
                wasm_path,
                javascript_path,
                size_bytes: entry_size,
                last_used: now,
                protected_until: now + self.grace,
                pinned: true,
            },
        );
        Ok(())
    }

    /// Reads the WASM artifact while the caller's cache lock remains held, updating its LRU time.
    pub async fn download(&mut self, key: &str) -> io::Result<Option<Vec<u8>>> {
        self.download_file(key, false).await
    }

    /// Reads the generated JavaScript glue while the caller's cache lock remains held.
    pub async fn download_javascript(&mut self, key: &str) -> io::Result<Option<Vec<u8>>> {
        self.download_file(key, true).await
    }

    async fn download_file(&mut self, key: &str, javascript: bool) -> io::Result<Option<Vec<u8>>> {
        let Some(entry) = self.entries.get(key) else {
            return Ok(None);
        };
        let artifact_path = if javascript {
            entry.javascript_path.clone()
        } else {
            entry.wasm_path.clone()
        };
        let result = tokio::fs::read(artifact_path).await;
        match result {
            Ok(bytes) => {
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.last_used = Instant::now();
                }
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // The in-memory index is authoritative; drop stale entries if files vanished.
                self.entries.remove(key);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn plan_evictions(&self, key: &str, entry_size: u64) -> Result<Vec<String>, CacheError> {
        if entry_size > self.max_bytes || self.max_entries == 0 {
            return Err(CacheError::Capacity);
        }

        let now = Instant::now();
        let mut planned_bytes = self
            .entries
            .values()
            .fold(0_u64, |total, entry| total.saturating_add(entry.size_bytes));
        let mut planned_entries = self.entries.len();
        let mut candidates: Vec<_> = self
            .entries
            .iter()
            .filter(|(candidate_key, entry)| {
                candidate_key.as_str() != key && !entry.pinned && entry.protected_until <= now
            })
            .map(|(candidate_key, entry)| (candidate_key.clone(), entry.last_used))
            .collect();
        candidates.sort_by_key(|(_, last_used)| *last_used);

        let mut victims = Vec::new();
        for (candidate_key, _) in candidates {
            if planned_bytes <= self.max_bytes - entry_size && planned_entries < self.max_entries {
                break;
            }
            let victim_size = self.entries[&candidate_key].size_bytes;
            planned_bytes = planned_bytes.saturating_sub(victim_size);
            planned_entries = planned_entries.saturating_sub(1);
            victims.push(candidate_key);
        }

        if planned_bytes > self.max_bytes - entry_size || planned_entries >= self.max_entries {
            return Err(CacheError::Capacity);
        }
        Ok(victims)
    }

    async fn evict_entries(&mut self, victims: Vec<String>) -> io::Result<()> {
        for victim_key in victims {
            let Some(entry) = self.entries.get(&victim_key) else {
                continue;
            };
            if entry.pinned {
                continue;
            }
            tokio::fs::remove_dir_all(self.entry_dir(&victim_key)).await?;
            self.entries.remove(&victim_key);
            tracing::debug!(artifact_id = %victim_key, "Evicted cached artifact");
        }
        Ok(())
    }

    fn entry_dir(&self, key: &str) -> PathBuf {
        self.namespace.path().join(key)
    }
}

fn digest_hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(code: &str) -> CompileRequest {
        CompileRequest {
            code: code.to_owned(),
        }
    }

    fn metadata(key: &str) -> CompileResponse {
        CompileResponse {
            artifact_url: Some(format!("/artifacts/{key}.wasm")),
            compilation_time_ms: 42,
            warnings: Vec::new(),
            errors: Vec::new(),
            cache_hit: false,
        }
    }

    #[tokio::test]
    async fn keys_cover_request_and_startup_namespace() {
        let root = tempfile::tempdir().unwrap();
        let a = ArtifactCache::new(root.path(), 4096, 4, Duration::ZERO)
            .await
            .unwrap();
        let b = ArtifactCache::new(root.path(), 4096, 4, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(a.key(&request("a")), a.key(&request("a")));
        assert_ne!(a.key(&request("a")), a.key(&request("b")));
        assert_ne!(a.key(&request("a")), b.key(&request("a")));
    }

    #[tokio::test]
    async fn grace_protects_new_entries_and_hits_refresh_it() {
        let root = tempfile::tempdir().unwrap();
        let grace = Duration::from_secs(300);
        let mut cache = ArtifactCache::new(root.path(), 4096, 1, grace)
            .await
            .unwrap();
        let a = cache.key(&request("a"));
        let b = cache.key(&request("b"));
        cache
            .insert(a.clone(), b"wasm", b"js", metadata(&a), false)
            .await
            .unwrap();
        assert!(matches!(
            cache
                .insert(b.clone(), b"other", b"js", metadata(&b), false)
                .await,
            Err(CacheError::Capacity)
        ));
        cache.entries.get_mut(&a).unwrap().protected_until = Instant::now();
        let response = cache.lookup(&a).await.unwrap();
        assert!(response.cache_hit);
        assert_eq!(response.compilation_time_ms, 42);
        assert!(cache.entries[&a].protected_until > Instant::now());
        assert!(matches!(
            cache
                .insert(b.clone(), b"other", b"js", metadata(&b), false)
                .await,
            Err(CacheError::Capacity)
        ));
        cache.entries.get_mut(&a).unwrap().protected_until = Instant::now();
        cache
            .insert(b.clone(), b"other", b"js", metadata(&b), false)
            .await
            .unwrap();
        assert!(cache.lookup(&a).await.is_none());
        assert_eq!(cache.download(&b).await.unwrap(), Some(b"other".to_vec()));
    }

    #[tokio::test]
    async fn evicts_lru_but_not_pinned_entries() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = ArtifactCache::new(root.path(), 4096, 3, Duration::ZERO)
            .await
            .unwrap();
        let pinned = cache.key(&request("default"));
        let a = cache.key(&request("a"));
        let b = cache.key(&request("b"));
        let c = cache.key(&request("c"));
        for (key, pinned) in [(&pinned, true), (&a, false), (&b, false)] {
            cache
                .insert(key.clone(), b"wasm", b"js", metadata(key), pinned)
                .await
                .unwrap();
        }
        // Make ordering explicit rather than depending on clock resolution.
        cache.entries.get_mut(&a).unwrap().last_used = Instant::now() - Duration::from_secs(10);
        cache.download(&b).await.unwrap();
        cache
            .insert(c.clone(), b"wasm", b"js", metadata(&c), false)
            .await
            .unwrap();
        assert!(cache.lookup(&a).await.is_none());
        assert!(cache.lookup(&pinned).await.is_some());
        assert!(cache.lookup(&b).await.is_some());
        assert!(cache.lookup(&c).await.is_some());
        assert_eq!(cache.download("../../etc/passwd").await.unwrap(), None);
    }

    #[tokio::test]
    async fn precompiled_entries_reference_bundled_files_and_survive_eviction_and_drop() {
        let root = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let wasm_path = bundled.path().join("artifact.wasm");
        tokio::fs::write(&wasm_path, b"bundled wasm").await.unwrap();
        let javascript_path = bundled.path().join("artifact.js");
        tokio::fs::write(&javascript_path, b"bundled js")
            .await
            .unwrap();
        let compile_request = request("precompiled");
        let compilation_metadata = CompilationMetadata {
            compilation_time_ms: 17,
            warnings: Vec::new(),
        };
        let mut cache = ArtifactCache::new(root.path(), 4096, 2, Duration::ZERO)
            .await
            .unwrap();
        let key = cache.key(&compile_request);

        cache
            .register_precompiled(&compile_request, wasm_path.clone(), compilation_metadata)
            .await
            .unwrap();
        assert_eq!(cache.entries[&key].wasm_path, wasm_path);
        assert_eq!(cache.entries[&key].javascript_path, javascript_path);
        assert!(!cache.entry_dir(&key).join("artifact.wasm").exists());
        assert!(!cache.entry_dir(&key).join("artifact.js").exists());
        let response = cache.lookup(&key).await.unwrap();
        assert_eq!(
            response.artifact_url,
            Some(format!("/artifacts/{key}.wasm"))
        );
        assert!(response.cache_hit);
        assert_eq!(response.compilation_time_ms, 17);
        assert_eq!(
            cache.download(&key).await.unwrap(),
            Some(b"bundled wasm".to_vec())
        );
        assert_eq!(
            cache.download_javascript(&key).await.unwrap(),
            Some(b"bundled js".to_vec())
        );

        let runtime_request = request("runtime");
        let runtime_key = cache.key(&runtime_request);
        cache
            .insert(
                runtime_key.clone(),
                b"runtime wasm",
                b"runtime js",
                metadata(&runtime_key),
                false,
            )
            .await
            .unwrap();
        cache.entries.get_mut(&runtime_key).unwrap().last_used =
            Instant::now() - Duration::from_secs(10);
        let next_request = request("next");
        let next_key = cache.key(&next_request);
        cache
            .insert(
                next_key.clone(),
                b"next wasm",
                b"next js",
                metadata(&next_key),
                false,
            )
            .await
            .unwrap();
        assert!(cache.entries.contains_key(&key));
        assert!(!cache.entries.contains_key(&runtime_key));
        drop(cache);
        assert_eq!(tokio::fs::read(&wasm_path).await.unwrap(), b"bundled wasm");
    }

    #[tokio::test]
    async fn precompiled_entries_require_a_javascript_sibling() {
        let root = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let wasm_path = bundled.path().join("artifact.wasm");
        tokio::fs::write(&wasm_path, b"wasm").await.unwrap();
        let mut cache = ArtifactCache::new(root.path(), 4096, 4, Duration::ZERO)
            .await
            .unwrap();
        let error = cache
            .register_precompiled(
                &request("precompiled"),
                wasm_path,
                CompilationMetadata {
                    compilation_time_ms: 17,
                    warnings: Vec::new(),
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, CacheError::Io(ref error) if error.kind() == io::ErrorKind::NotFound)
        );
        assert!(cache.entries.is_empty());
    }

    #[tokio::test]
    async fn precompiled_entries_count_wasm_javascript_and_metadata_toward_capacity() {
        let root = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let wasm_path = bundled.path().join("artifact.wasm");
        tokio::fs::write(&wasm_path, b"wasm").await.unwrap();
        tokio::fs::write(bundled.path().join("artifact.js"), b"js")
            .await
            .unwrap();
        let metadata = CompilationMetadata {
            compilation_time_ms: 17,
            warnings: Vec::new(),
        };
        let metadata_size = to_vec(&metadata).unwrap().len() as u64;
        let budget = b"wasm".len() as u64 + b"js".len() as u64 + metadata_size - 1;
        let mut cache = ArtifactCache::new(root.path(), budget, 4, Duration::ZERO)
            .await
            .unwrap();
        assert!(matches!(
            cache
                .register_precompiled(&request("precompiled"), wasm_path, metadata)
                .await,
            Err(CacheError::Capacity)
        ));
        assert!(cache.entries.is_empty());
    }

    #[tokio::test]
    async fn counts_metadata_and_rejects_oversized_entries_without_eviction() {
        let root = tempfile::tempdir().unwrap();
        let mut cache = ArtifactCache::new(root.path(), 300, 10, Duration::ZERO)
            .await
            .unwrap();
        let a = cache.key(&request("a"));
        let b = cache.key(&request("b"));
        cache
            .insert(a.clone(), b"wasm", b"js", metadata(&a), false)
            .await
            .unwrap();
        assert!(matches!(
            cache
                .insert(b.clone(), &[0; 300], b"js", metadata(&b), false)
                .await,
            Err(CacheError::Capacity)
        ));
        assert!(cache.lookup(&a).await.is_some());
        // The artifacts fit, but the metadata files plus artifact bodies do not.
        cache.entries.get_mut(&a).unwrap().protected_until = Instant::now();
        cache
            .insert(b.clone(), b"wasm", b"js", metadata(&b), false)
            .await
            .unwrap();
        assert!(cache.lookup(&a).await.is_none());
        assert!(cache.lookup(&b).await.is_some());
    }
}
