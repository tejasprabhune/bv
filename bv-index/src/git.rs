use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use bv_core::data::DataManifest;
use bv_core::error::{BvError, Result};
use bv_core::manifest::Manifest;
use semver::{Version, VersionReq};

use crate::backend::{IndexBackend, ToolSummary};

pub struct GitIndex {
    pub url: String,
    pub local_path: PathBuf,
}

impl GitIndex {
    pub fn new(url: impl Into<String>, local_path: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            local_path: local_path.into(),
        }
    }

    /// Refresh only if the local clone is older than `ttl`.
    /// Returns `true` when an actual network fetch was performed.
    pub fn refresh_if_stale(&self, ttl: std::time::Duration) -> Result<bool> {
        let stamp = self.local_path.join(".bv-refresh");
        let is_fresh = stamp
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|elapsed| elapsed < ttl)
            .unwrap_or(false);

        if is_fresh {
            return Ok(false);
        }

        self.git_refresh()?;
        self.touch_stamp();
        Ok(true)
    }

    /// True when the local clone exists and has been fetched at least once.
    pub fn is_available(&self) -> bool {
        self.local_path.join(".bv-refresh").exists() || self.local_path.join(".git").exists()
    }

    /// Path to the local clone of this registry.
    pub fn local_path(&self) -> &std::path::Path {
        &self.local_path
    }

    fn git_refresh(&self) -> Result<()> {
        if self.local_path.exists() {
            let out = Command::new("git")
                .args([
                    "-C",
                    &self.local_path.to_string_lossy(),
                    "pull",
                    "--ff-only",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()?;
            if !out.status.success() {
                let msg = String::from_utf8_lossy(&out.stderr);
                return Err(BvError::IndexError(format!(
                    "git pull failed in {}: {}",
                    self.local_path.display(),
                    msg.trim()
                )));
            }
        } else {
            if let Some(parent) = self.local_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let out = Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    &self.url,
                    &self.local_path.to_string_lossy(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()?;
            if !out.status.success() {
                let msg = String::from_utf8_lossy(&out.stderr);
                return Err(BvError::IndexError(format!(
                    "git clone failed for '{}': {}",
                    self.url,
                    msg.trim()
                )));
            }
        }
        Ok(())
    }

    fn touch_stamp(&self) {
        let stamp = self.local_path.join(".bv-refresh");
        let _ = fs::write(&stamp, "");
    }
}

impl IndexBackend for GitIndex {
    fn name(&self) -> &str {
        "git"
    }

    fn refresh(&self) -> Result<()> {
        self.git_refresh()?;
        self.touch_stamp();
        Ok(())
    }

    fn get_manifest(&self, tool: &str, version: &VersionReq) -> Result<Manifest> {
        let tool_dir = self.local_path.join("tools").join(tool);
        if !tool_dir.exists() {
            return Err(BvError::IndexError(format!(
                "tool '{tool}' not found in registry"
            )));
        }

        let versions = self.list_versions(tool)?;
        if versions.is_empty() {
            return Err(BvError::IndexError(format!(
                "no versions of '{tool}' found in registry"
            )));
        }

        let best = versions
            .iter()
            .filter(|v| version.matches(v))
            .max()
            .ok_or_else(|| {
                BvError::IndexError(format!(
                    "no version of '{tool}' satisfies '{version}' (available: {})",
                    versions
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;

        let manifest_path = tool_dir.join(format!("{best}.toml"));
        let s = fs::read_to_string(&manifest_path).map_err(|e| {
            BvError::IndexError(format!("could not read manifest for '{tool}@{best}': {e}"))
        })?;

        Manifest::from_toml_str(&s)
    }

    fn list_versions(&self, tool: &str) -> Result<Vec<Version>> {
        let tool_dir = self.local_path.join("tools").join(tool);
        if !tool_dir.exists() {
            return Err(BvError::IndexError(format!(
                "tool '{tool}' not found in registry"
            )));
        }

        let mut versions = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for entry in fs::read_dir(&tool_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                match stem.parse::<Version>() {
                    Ok(v) => versions.push(v),
                    Err(_) => dropped.push(stem.to_string()),
                }
            }
        }

        if !dropped.is_empty() {
            tracing::warn!(
                tool = %tool,
                dropped = ?dropped,
                "ignoring tool manifest files with non-semver names (expected MAJOR.MINOR.PATCH; \
                 calver like 2024.01.0 is not valid semver)"
            );
        }

        versions.sort();
        Ok(versions)
    }

    fn list_tools(&self) -> Result<Vec<ToolSummary>> {
        let tools_dir = self.local_path.join("tools");
        if !tools_dir.exists() {
            return Ok(vec![]);
        }

        let mut tools = Vec::new();
        for entry in fs::read_dir(&tools_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let id = entry.file_name().to_string_lossy().to_string();
            let versions = self.list_versions(&id).unwrap_or_default();

            let latest_manifest = versions.last().and_then(|v| {
                let p = tools_dir.join(&id).join(format!("{v}.toml"));
                fs::read_to_string(p)
                    .ok()
                    .and_then(|s| Manifest::from_toml_str(&s).ok())
            });

            let description = latest_manifest
                .as_ref()
                .and_then(|m| m.tool.description.clone());
            let tier = latest_manifest
                .as_ref()
                .map(|m| m.tool.tier.clone())
                .unwrap_or_default();
            let deprecated = latest_manifest
                .as_ref()
                .map(|m| m.tool.deprecated)
                .unwrap_or(false);
            let input_types = latest_manifest
                .as_ref()
                .map(|m| m.tool.inputs.iter().map(|i| i.r#type.to_string()).collect())
                .unwrap_or_default();
            let output_types = latest_manifest
                .as_ref()
                .map(|m| {
                    m.tool
                        .outputs
                        .iter()
                        .map(|o| o.r#type.to_string())
                        .collect()
                })
                .unwrap_or_default();

            tools.push(ToolSummary {
                id,
                latest_version: versions.last().map(|v| v.to_string()).unwrap_or_default(),
                description,
                tier,
                deprecated,
                input_types,
                output_types,
            });
        }

        tools.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tools)
    }

    fn get_data_manifest(&self, dataset: &str, version: Option<&str>) -> Result<DataManifest> {
        let data_dir = self.local_path.join("data").join(dataset);
        if !data_dir.exists() {
            return Err(BvError::IndexError(format!(
                "dataset '{dataset}' not found in registry"
            )));
        }

        let ver = if let Some(v) = version {
            v.to_string()
        } else {
            let mut versions = self.list_data_versions(dataset)?;
            versions.sort();
            versions.into_iter().last().ok_or_else(|| {
                BvError::IndexError(format!("no versions of '{dataset}' found in registry"))
            })?
        };

        let manifest_path = data_dir.join(format!("{ver}.toml"));
        let s = fs::read_to_string(&manifest_path).map_err(|e| {
            BvError::IndexError(format!(
                "could not read data manifest for '{dataset}@{ver}': {e}"
            ))
        })?;

        DataManifest::from_toml_str(&s)
    }

    fn list_data_versions(&self, dataset: &str) -> Result<Vec<String>> {
        let data_dir = self.local_path.join("data").join(dataset);
        if !data_dir.exists() {
            return Err(BvError::IndexError(format!(
                "dataset '{dataset}' not found in registry"
            )));
        }

        let mut versions = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for entry in fs::read_dir(&data_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                if stem.parse::<Version>().is_ok() {
                    versions.push(stem.to_string());
                } else {
                    dropped.push(stem.to_string());
                }
            }
        }

        if !dropped.is_empty() {
            tracing::warn!(
                dataset = %dataset,
                dropped = ?dropped,
                "ignoring dataset manifest files with non-semver names (expected MAJOR.MINOR.PATCH; \
                 calver like 2024.01.0 is not valid semver)"
            );
        }

        versions.sort();
        Ok(versions)
    }

    fn list_datasets(&self) -> Result<Vec<String>> {
        let data_dir = self.local_path.join("data");
        if !data_dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(&data_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|s| s.to_str())
            {
                ids.push(name.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_versions_returns_only_valid_semver() {
        let tmp = tempdir().unwrap();
        let tool_dir = tmp.path().join("tools").join("tmalign");
        fs::create_dir_all(&tool_dir).unwrap();
        fs::write(tool_dir.join("1.2.3.toml"), "").unwrap();
        fs::write(tool_dir.join("20240303.toml"), "").unwrap();
        fs::write(tool_dir.join("2024.01.0.toml"), "").unwrap();

        let index = GitIndex::new("unused", tmp.path().to_path_buf());
        let versions = index.list_versions("tmalign").unwrap();

        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0], Version::new(1, 2, 3));
    }

    #[test]
    fn list_data_versions_returns_only_valid_semver() {
        let tmp = tempdir().unwrap();
        let data_dir = tmp.path().join("data").join("uniref50");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("0.1.0.toml"), "").unwrap();
        fs::write(data_dir.join("2024.01.0.toml"), "").unwrap();

        let index = GitIndex::new("unused", tmp.path().to_path_buf());
        let versions = index.list_data_versions("uniref50").unwrap();

        assert_eq!(versions, vec!["0.1.0".to_string()]);
    }
}
