use std::path::PathBuf;

use directories::ProjectDirs;

/// On-disk layout for bv's local cache.
pub struct CacheLayout {
    root: PathBuf,
}

impl CacheLayout {
    /// Resolve the cache root using platform conventions.
    /// Falls back to `~/.cache/bv` if XDG/platform dirs are unavailable.
    pub fn new() -> Self {
        let root = ProjectDirs::from("io", "bv", "bv")
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(|| dirs_fallback().join("bv"));
        Self { root }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// `<root>/images/<digest>/` - metadata we track alongside runtime storage.
    pub fn image_dir(&self, digest: &str) -> PathBuf {
        self.root.join("images").join(digest)
    }

    /// `<root>/tools/<tool_id>/<version>/manifest.toml`
    pub fn manifest_path(&self, tool_id: &str, version: &str) -> PathBuf {
        self.root
            .join("tools")
            .join(tool_id)
            .join(version)
            .join("manifest.toml")
    }

    /// `<root>/index/<index_name>/` - local clones of git registries.
    pub fn index_dir(&self, index_name: &str) -> PathBuf {
        self.root.join("index").join(index_name)
    }

    /// `<root>/data/<dataset_id>/<version>/`
    pub fn data_dir(&self, dataset_id: &str, version: &str) -> PathBuf {
        self.root.join("data").join(dataset_id).join(version)
    }

    /// `<root>/tmp/` - staging area for atomic writes.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

impl Default for CacheLayout {
    fn default() -> Self {
        Self::new()
    }
}

fn dirs_fallback() -> PathBuf {
    dirs_sys_home().unwrap_or_else(|| PathBuf::from("."))
}

fn dirs_sys_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
