//! Image-bundled examples. Build mode and normal startup share this registry.

use std::{io, path::Path, time::Duration};

use sycamore_playground_shared::{CompileRequest, DEFAULT_CODE};

use crate::{
    cache::ArtifactCache,
    compiler::{CompilationMetadata, Compiler},
};

const EXAMPLES: &[(&str, &str)] = &[("default", DEFAULT_CODE)];

/// Run offline (not alongside the server): compile every registered example and
/// publish the bundle only after all compilations and writes have succeeded.
/// Unlike HTTP requests, image builds allow time for cold dependency compilation.
pub(crate) async fn build(
    cache_root: &Path,
    compiler: &Compiler,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::fs::create_dir_all(cache_root).await?;
    let staging = tempfile::Builder::new()
        .prefix(".precompile-")
        .tempdir_in(cache_root)?;
    for &(name, code) in EXAMPLES {
        tracing::info!(example = name, "Precompiling example");
        let request = CompileRequest {
            code: code.to_owned(),
        };
        let artifact = compiler.compile(&request, Duration::from_secs(300)).await?;
        let directory = staging.path().join(name);
        tokio::fs::create_dir(&directory).await?;
        tokio::fs::write(
            directory.join("request.json"),
            serde_json::to_vec(&request)?,
        )
        .await?;
        tokio::fs::write(
            directory.join("metadata.json"),
            serde_json::to_vec(&artifact.metadata)?,
        )
        .await?;
        tokio::fs::write(directory.join("artifact.wasm"), artifact.wasm).await?;
        tokio::fs::write(directory.join("artifact.js"), artifact.javascript).await?;
    }

    let destination = cache_root.join("precompiled");
    match tokio::fs::remove_dir_all(&destination).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    tokio::fs::rename(staging.path(), &destination).await?;
    tracing::info!(
        example_count = EXAMPLES.len(),
        directory = %destination.display(),
        "Precompiled bundle built"
    );
    Ok(())
}

/// Load metadata only; the cache references the immutable bundled artifacts in place.
/// A missing bundle is allowed for local development, with lazy compilation on
/// the first request. Incomplete or stale bundles fail startup rather than serve
/// an artifact that does not match the binary's registered request. Older bundles without
/// `artifact.js` must be rebuilt with `--precompile`.
pub(crate) async fn register(cache_root: &Path, cache: &mut ArtifactCache) -> io::Result<()> {
    let root = cache_root.join("precompiled");
    match tokio::fs::metadata(&root).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(io::Error::other("precompiled path is not a directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::warn!("No precompiled bundle; run with --precompile to build it");
            return Ok(());
        }
        Err(error) => return Err(error),
    }

    for &(name, code) in EXAMPLES {
        let directory = root.join(name);
        let request: CompileRequest =
            serde_json::from_slice(&tokio::fs::read(directory.join("request.json")).await?)?;
        if request.code != code {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Precompiled {name} is stale; rebuild with --precompile"),
            ));
        }
        let metadata: CompilationMetadata =
            serde_json::from_slice(&tokio::fs::read(directory.join("metadata.json")).await?)?;
        cache
            .register_precompiled(&request, directory.join("artifact.wasm"), metadata)
            .await
            .map_err(io::Error::other)?;
        tracing::info!(example = name, "Registered pinned precompiled artifact");
    }
    Ok(())
}
