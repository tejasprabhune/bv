use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Persistent index that maps OCI layer digests to the SIF digest that
/// contains them.
///
/// Stored as JSON at `<sif_dir>/layer-index.json`. Each entry records that
/// a particular OCI layer was present in a specific SIF. When `ensure_layers`
/// checks whether a set of layers is already cached, it consults this index
/// to find a matching SIF without re-pulling the image.
///
/// This is the on-disk dedup primitive for the Apptainer backend: two tools
/// that share a layer (by digest) will both record a pointer to the same SIF,
/// and `ensure_layers` can short-circuit the pull when the SIF is present.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayerIndex {
    pub version: u32,
    /// OCI layer digest → SIF digest.
    pub entries: HashMap<String, String>,
}

impl LayerIndex {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self {
                version: 1,
                entries: HashMap::new(),
            })
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// Record that `layer_digest` is present in the SIF identified by `sif_digest`.
    pub fn record(&mut self, layer_digest: &str, sif_digest: &str) {
        self.entries.insert(layer_digest.to_string(), sif_digest.to_string());
    }

    /// Return the SIF digest that contains `layer_digest`, if known.
    pub fn sif_for_layer(&self, layer_digest: &str) -> Option<&str> {
        self.entries.get(layer_digest).map(|s| s.as_str())
    }
}

/// Default path for the layer index within a SIF cache directory.
pub fn layer_index_path(sif_dir: &Path) -> PathBuf {
    sif_dir.join("layer-index.json")
}

/// Parse a version string like `"1.3.4-1.el9"` or `"4.1.0"` into `(major, minor)`.
///
/// Returns `None` if parsing fails; callers should treat unknown versions as
/// supported to avoid false negatives.
pub fn parse_version_major_minor(version_str: &str) -> Option<(u32, u32)> {
    let trimmed = version_str.trim();
    let parts: Vec<&str> = trimmed.splitn(3, '.').collect();
    if parts.len() < 2 {
        return None;
    }
    let major = parts[0].parse::<u32>().ok()?;
    let minor_str = parts[1].split(|c: char| !c.is_ascii_digit()).next()?;
    let minor = minor_str.parse::<u32>().ok()?;
    Some((major, minor))
}

/// Return `true` if the Apptainer/Singularity binary supports OCI-native mode.
///
/// OCI-native mode (required for factored image pulls) was stabilized in
/// Apptainer 1.2.0 and Singularity 4.x. On older versions, `assemble_image`
/// still works but falls back to standard `apptainer pull docker://...`,
/// which converts to SIF without per-layer optimisation.
pub fn supports_oci_native(bin: &str) -> bool {
    let Ok(out) = std::process::Command::new(bin).arg("version").output() else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let version_str = String::from_utf8_lossy(&out.stdout);
    parse_version_major_minor(&version_str)
        .map(|(major, minor)| major > 1 || (major == 1 && minor >= 2))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_major_minor_standard() {
        assert_eq!(parse_version_major_minor("1.3.4"), Some((1, 3)));
        assert_eq!(parse_version_major_minor("4.1.0"), Some((4, 1)));
        assert_eq!(parse_version_major_minor("1.2.0"), Some((1, 2)));
    }

    #[test]
    fn parse_version_with_suffix() {
        assert_eq!(parse_version_major_minor("1.3.4-1.el9"), Some((1, 3)));
        assert_eq!(parse_version_major_minor("4.0.1+dfsg-1"), Some((4, 0)));
    }

    #[test]
    fn parse_version_edge_cases() {
        assert_eq!(parse_version_major_minor(""), None);
        assert_eq!(parse_version_major_minor("notaversion"), None);
    }

    #[test]
    fn layer_index_record_and_lookup() {
        let mut idx = LayerIndex::default();
        idx.record("sha256:aaa", "sha256:sif1");
        idx.record("sha256:bbb", "sha256:sif1");
        assert_eq!(idx.sif_for_layer("sha256:aaa"), Some("sha256:sif1"));
        assert_eq!(idx.sif_for_layer("sha256:bbb"), Some("sha256:sif1"));
        assert_eq!(idx.sif_for_layer("sha256:unknown"), None);
    }

    #[test]
    fn layer_index_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layer-index.json");

        let mut idx = LayerIndex { version: 1, entries: HashMap::new() };
        idx.record("sha256:layer1", "sha256:sif1");
        idx.save(&path).unwrap();

        let loaded = LayerIndex::load(&path).unwrap();
        assert_eq!(loaded.sif_for_layer("sha256:layer1"), Some("sha256:sif1"));
    }
}
