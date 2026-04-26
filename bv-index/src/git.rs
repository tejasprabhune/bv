use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
}

impl IndexBackend for GitIndex {
    fn name(&self) -> &str {
        "git"
    }

    fn refresh(&self) -> Result<()> {
        if self.local_path.exists() {
            let out = Command::new("git")
                .args(["-C", &self.local_path.to_string_lossy(), "pull", "--ff-only"])
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
                .args(["clone", "--depth", "1", &self.url, &self.local_path.to_string_lossy()])
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
            BvError::IndexError(format!(
                "could not read manifest for '{tool}@{best}': {e}"
            ))
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
        for entry in fs::read_dir(&tool_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(v) = stem.parse::<Version>()
            {
                versions.push(v);
            }
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

            // Pull description from the latest version's manifest.
            let description = versions.last().and_then(|v| {
                let p = tools_dir.join(&id).join(format!("{v}.toml"));
                fs::read_to_string(p)
                    .ok()
                    .and_then(|s| Manifest::from_toml_str(&s).ok())
                    .and_then(|m| m.tool.description)
            });

            tools.push(ToolSummary {
                id,
                latest_version: versions.last().map(|v| v.to_string()).unwrap_or_default(),
                description,
            });
        }

        tools.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tools)
    }
}
