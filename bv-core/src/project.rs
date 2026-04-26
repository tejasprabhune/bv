use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};
use crate::lockfile::Lockfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub id: String,
    /// SemVer version requirement, e.g. `">=0.7,<1"`. Empty means "latest".
    #[serde(default)]
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

/// Contents of `bv.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BvToml {
    pub project: ProjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistryConfig>,
    #[serde(default)]
    pub tools: Vec<ToolDeclaration>,
    #[serde(default)]
    pub data: HashMap<String, DataDeclaration>,
    #[serde(default)]
    pub hardware: HardwareProfile,
}

impl BvToml {
    pub fn from_path(path: &Path) -> Result<Self> {
        let s = fs::read_to_string(path)?;
        toml::from_str(&s).map_err(|e| BvError::ManifestParse(e.to_string()))
    }

    pub fn to_path(&self, path: &Path) -> Result<()> {
        let s =
            toml::to_string_pretty(self).map_err(|e| BvError::ManifestParse(e.to_string()))?;
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
