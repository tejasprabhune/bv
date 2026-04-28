use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PostDownloadAction {
    #[default]
    Noop,
    Extract,
    Decompress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataEntry {
    pub id: String,
    pub version: String,
    pub description: Option<String>,
    pub source_urls: Vec<String>,
    /// Expected SHA-256 of the primary downloaded file as `sha256:<hex>`.
    pub sha256: String,
    pub size_bytes: u64,
    /// File format hint, e.g. `"tar"`, `"fasta_gz"`, `"raw"`.
    pub format: String,
    #[serde(default)]
    pub post_download_action: PostDownloadAction,
    pub license: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataManifest {
    pub data: DataEntry,
}

impl DataManifest {
    pub fn from_toml_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| BvError::ManifestParse(e.to_string()))
    }
}
