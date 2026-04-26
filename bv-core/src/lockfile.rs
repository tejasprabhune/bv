use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};

/// Per-dataset pin stored inside a lockfile entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceDataPin {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

/// One resolved tool entry in `bv.lock`.
///
/// Stability fields used by `bv lock --check` to detect drift:
/// `tool_id`, `version`, `image_digest`, `manifest_sha256`.
/// Timestamps and sizes are informational only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileEntry {
    pub tool_id: String,
    /// Version requirement as declared in `bv.toml` (e.g. `=2.14.0`, `^2`, or `*`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub declared_version_req: String,
    /// Resolved semver (e.g. `2.14.0`).
    pub version: String,
    /// Canonical OCI reference from the manifest (e.g. `ncbi/blast:2.14.0`).
    pub image_reference: String,
    /// Content digest of the pulled image (e.g. `sha256:abc123...`).
    pub image_digest: String,
    /// SHA-256 of the manifest TOML at resolve time; used for drift detection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_sha256: String,
    pub image_size_bytes: Option<u64>,
    pub resolved_at: DateTime<Utc>,
    #[serde(default)]
    pub reference_data_pins: HashMap<String, ReferenceDataPin>,
}

impl LockfileEntry {
    /// True when two entries represent the same resolved state.
    /// Ignores timestamps, sizes, and declared_version_req.
    pub fn is_equivalent(&self, other: &Self) -> bool {
        self.tool_id == other.tool_id
            && self.version == other.version
            && self.image_digest == other.image_digest
            && (self.manifest_sha256.is_empty()
                || other.manifest_sha256.is_empty()
                || self.manifest_sha256 == other.manifest_sha256)
    }
}

/// Informational metadata written to `bv.lock` by `bv lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileMetadata {
    pub bv_version: String,
    pub generated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_summary: Option<String>,
}

impl Default for LockfileMetadata {
    fn default() -> Self {
        Self {
            bv_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: Utc::now(),
            hardware_summary: None,
        }
    }
}

/// The full `bv.lock` file (schema version 1).
///
/// Format is stable: `bv lock --check` fails if the generated lockfile
/// would differ from the on-disk one on any stability field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Schema version; currently always `1`.
    pub version: u32,
    #[serde(default)]
    pub metadata: LockfileMetadata,
    #[serde(default)]
    pub tools: HashMap<String, LockfileEntry>,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            version: 1,
            metadata: LockfileMetadata::default(),
            tools: HashMap::new(),
        }
    }

    pub fn from_toml_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| BvError::LockfileParse(e.to_string()))
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| BvError::LockfileParse(e.to_string()))
    }

    /// True when both lockfiles describe the same set of tools at the same
    /// resolved versions and digests.
    pub fn is_equivalent_to(&self, other: &Self) -> bool {
        if self.tools.len() != other.tools.len() {
            return false;
        }
        for (id, entry) in &self.tools {
            match other.tools.get(id) {
                Some(other_entry) => {
                    if !entry.is_equivalent(other_entry) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}
