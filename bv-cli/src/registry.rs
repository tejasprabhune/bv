use std::path::Path;

use bv_core::cache::CacheLayout;
use bv_index::GitIndex;

/// The built-in default registry. Users can override with BV_REGISTRY or bv.toml.
pub const DEFAULT_REGISTRY: &str = "https://github.com/mlberkeley/bv-registry";

/// Resolve the registry URL from (in priority order):
/// 1. explicit flag / arg
/// 2. BV_REGISTRY environment variable
/// 3. [registry] url in bv.toml (if bv_toml is provided)
/// 4. Built-in default
pub fn resolve_registry_url(
    flag: Option<&str>,
    bv_toml: Option<&bv_core::project::BvToml>,
) -> String {
    flag.map(|s| s.to_string())
        .or_else(|| std::env::var("BV_REGISTRY").ok())
        .or_else(|| {
            bv_toml
                .and_then(|t| t.registry.as_ref())
                .map(|r| r.url.clone())
        })
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
}

/// Build a GitIndex for the given URL, using the shared on-disk clone at
/// `<cache>/index/default/`.
pub fn open_index(url: &str, cache: &CacheLayout) -> GitIndex {
    GitIndex::new(url, cache.index_dir("default"))
}

/// Five-minute TTL used for implicit refreshes (bv data fetch, bv sync drift check).
pub const STALE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Print "Updating index... done" only when a real network fetch occurred.
pub fn maybe_print_refresh(refreshed: bool) {
    use owo_colors::{OwoColorize, Stream};
    if refreshed {
        eprintln!(
            "  {} index",
            "Updated".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        );
    }
}

/// Require that the local index clone exists, giving a helpful error otherwise.
pub fn require_index(index: &GitIndex, _registry_url: &str) -> anyhow::Result<()> {
    if !index.is_available() {
        anyhow::bail!(
            "registry index not cloned yet\n  \
             Run `bv add <tool>` to clone the registry and resolve your tools\n  \
             Or if you already have a bv.lock, run `bv sync` to pull images directly"
        );
    }
    Ok(())
}

/// Look up git remote URL for `bv doctor`.
pub fn git_remote_url(repo_path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args([
            "-C",
            &repo_path.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
