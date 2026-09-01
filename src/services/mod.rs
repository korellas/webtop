//! The services panel's data layer.
//!
//! `manifest` reads what the operator declared; `probe` measures what is
//! actually happening. Nothing here knows the name of any particular service —
//! see the module docs on `manifest` for why that boundary is load-bearing.

pub mod manifest;
pub mod probe;

use crate::config::dirs_home;
use std::path::PathBuf;
use std::sync::RwLock;

/// The manifest, re-read on demand rather than cached for the process
/// lifetime. Editing the manifest and reloading the page should show the
/// change — a dashboard that needs restarting to notice a config edit is a
/// dashboard people stop trusting.
///
/// The parse is a few microseconds against a file of this size, but it is
/// still file I/O on the request path, so the result is memoised until the
/// file's mtime moves.
pub struct ManifestSource {
    path: PathBuf,
    cache: RwLock<Option<(std::time::SystemTime, Vec<manifest::ServiceDef>)>>,
}

impl ManifestSource {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: RwLock::new(None),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Current service definitions, reloading if the file changed on disk.
    pub fn load(&self) -> Result<Vec<manifest::ServiceDef>, String> {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .map_err(|e| format!("could not stat {}: {e}", self.path.display()))?;

        if let Some((cached_mtime, defs)) = crate::sync::guard(self.cache.read()).as_ref() {
            if *cached_mtime == mtime {
                return Ok(defs.clone());
            }
        }

        let defs = manifest::load(&self.path)?;
        *crate::sync::guard(self.cache.write()) = Some((mtime, defs.clone()));
        Ok(defs)
    }
}

/// Where the manifest lives when `--services-manifest` is not given.
///
/// A path inside webtop's own directory, symlinked by the owning stack's
/// installer. That keeps the default free of any particular repo layout while
/// still requiring zero configuration on the machine that has one.
pub fn default_manifest_path() -> PathBuf {
    PathBuf::from(format!("{}/.webtop/services.json", dirs_home()))
}
