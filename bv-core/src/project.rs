use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};
use crate::lockfile::Lockfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub id: String,
    /// SemVer version requirement, e.g. `">=0.7,<1"`. Omitted means "latest".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataDeclaration {
    pub id: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HardwareProfile {
    pub gpu: Option<bool>,
    pub cpu_cores: Option<u32>,
    pub ram_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub url: String,
}

/// `[runtime]` block in `bv.toml`. Selects the container backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeConfig {
    /// `"docker"`, `"apptainer"`, or `"auto"` (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

/// A writable cache directory bound into the container at runtime.
///
/// Used to persist tool scratch state (model weights, downloaded indices,
/// etc.) across runs, and to satisfy tools that write inside the image,
/// which apptainer's read-only SIF would otherwise reject.
///
/// ```toml
/// [[cache]]
/// match = "*"                       # tool id, or "*" for all tools
/// container_path = "/cache"
/// host_path = "~/.cache/bv/{tool}"  # `{tool}` is replaced with the tool id
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMount {
    /// Tool id this cache applies to. `"*"` matches every tool.
    #[serde(rename = "match", default = "default_match")]
    pub tool_match: String,
    pub container_path: String,
    pub host_path: String,
}

fn default_match() -> String {
    "*".to_string()
}

/// Contents of `bv.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BvToml {
    pub project: ProjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistryConfig>,
    #[serde(default)]
    pub tools: Vec<ToolDeclaration>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub data: HashMap<String, DataDeclaration>,
    #[serde(default, skip_serializing_if = "hardware_profile_is_default")]
    pub hardware: HardwareProfile,
    #[serde(default, skip_serializing_if = "runtime_config_is_default")]
    pub runtime: RuntimeConfig,
    /// Resolves collisions when two tools expose the same binary name.
    /// Maps binary name to the tool id that should own the shim.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub binary_overrides: HashMap<String, String>,
    /// User-declared cache mounts, applied to every `bv run` invocation
    /// whose tool id matches `match`. Persists scratch state (model weights,
    /// downloaded indices) across runs. User entries override the host_path
    /// of any matching cache declared by the tool's manifest.
    #[serde(default, rename = "cache", skip_serializing_if = "Vec::is_empty")]
    pub caches: Vec<CacheMount>,
}

fn runtime_config_is_default(rc: &RuntimeConfig) -> bool {
    rc.backend.is_none()
}

fn hardware_profile_is_default(h: &HardwareProfile) -> bool {
    h.gpu.is_none() && h.cpu_cores.is_none() && h.ram_gb.is_none()
}

impl BvToml {
    pub fn from_path(path: &Path) -> Result<Self> {
        let s = fs::read_to_string(path)?;
        toml::from_str(&s).map_err(|e| BvError::ManifestParse(e.to_string()))
    }

    pub fn to_path(&self, path: &Path) -> Result<()> {
        let s = toml::to_string_pretty(self).map_err(|e| BvError::ManifestParse(e.to_string()))?;
        atomic_write(path, &s)
    }
}

pub struct BvLock;

impl BvLock {
    pub fn from_path(path: &Path) -> Result<Lockfile> {
        let s = fs::read_to_string(path)?;
        Lockfile::from_toml_str(&s)
    }

    pub fn to_path(lock: &Lockfile, path: &Path) -> Result<()> {
        let s = lock.to_toml_string()?;
        atomic_write(path, &s)
    }
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp = tmp_path(parent);
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn tmp_path(dir: &Path) -> PathBuf {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!(".bv-tmp-{id}"))
}
