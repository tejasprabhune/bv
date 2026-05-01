use std::path::PathBuf;

/// On-disk layout for bv's local cache.
pub struct CacheLayout {
    root: PathBuf,
}

impl CacheLayout {
    /// Resolve the cache root.
    ///
    /// Priority:
    /// 1. `BV_CACHE_DIR` env var (used by tests and CI for isolation)
    /// 2. `$XDG_CACHE_HOME/bv` if `XDG_CACHE_HOME` is set
    /// 3. `~/.cache/bv` (consistent across Linux and macOS)
    pub fn new() -> Self {
        if let Ok(dir) = std::env::var("BV_CACHE_DIR") {
            return Self::with_root(PathBuf::from(dir));
        }
        let root = xdg_cache_home().join("bv");
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

    /// `<root>/sif/` - Apptainer SIF image cache.
    pub fn sif_dir(&self) -> PathBuf {
        self.root.join("sif")
    }

    /// `<root>/tmp/` - staging area for atomic writes.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// `<root>/owned-images.txt` - persistent record of every image bv has pulled.
    /// Used by `bv cache prune` to identify bv-owned Docker images.
    pub fn owned_images_path(&self) -> PathBuf {
        self.root.join("owned-images.txt")
    }
}

impl Default for CacheLayout {
    fn default() -> Self {
        Self::new()
    }
}

fn xdg_cache_home() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(dir);
    }
    // Fall back to ~/.cache on all platforms.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache")
}
