use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Co-occurrence scores for conda packages across the full tool registry.
///
/// Key: package name. Value: how many tools in the registry include it.
/// Higher score = more popular = deserves its own OCI layer.
///
/// Scores are computed by `bv-builder pack` from the registry's `specs/` tree
/// and committed as `popularity.json`. Each per-tool build reads this file and
/// uses it to decide which packages get solo layers vs. the long-tail layer.
///
/// Stability guarantee: scores are keyed by package NAME only, not version.
/// A new version of an already-popular package (e.g. Python 3.11.6 replacing
/// 3.11.5) inherits the same popularity score and therefore the same layer
/// priority — which means it still gets a solo layer, just with a different
/// digest. This bounds layer-order churn when popular packages are upgraded.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PopularityMap {
    pub version: u32,
    /// Package name → co-occurrence count (number of tools that list it).
    pub packages: HashMap<String, u64>,
}

impl PopularityMap {
    pub fn new() -> Self {
        Self {
            version: 1,
            packages: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("read popularity map '{}'", path.display()))?;
        serde_json::from_str(&s)
            .with_context(|| format!("parse popularity map '{}'", path.display()))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, &json)?;
        Ok(())
    }

    /// Popularity score for a package, defaulting to 0 for unknowns.
    pub fn score(&self, package_name: &str) -> u64 {
        self.packages.get(package_name).copied().unwrap_or(0)
    }

    /// Record that all `package_names` appear together in one tool.
    pub fn record_tool(&mut self, package_names: &[String]) {
        for name in package_names {
            *self.packages.entry(name.clone()).or_insert(0) += 1;
        }
    }
}

/// Compute a popularity map from all tool spec directories under `specs_root`.
///
/// Walks `specs_root/**/*.toml`, parses each as a `BuildSpec`, and counts
/// how many specs declare each package name. The resulting map is sorted
/// deterministically (by name) inside `save()` via serde_json's map ordering.
pub fn compute_from_spec_dir(specs_root: &Path) -> anyhow::Result<PopularityMap> {
    let mut map = PopularityMap::new();

    for entry in walkdir(specs_root)? {
        let path = entry?;
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let s = std::fs::read_to_string(&path)
            .with_context(|| format!("read spec '{}'", path.display()))?;

        // Parse only the `packages` field; skip unrelated TOML.
        let raw: toml::Value = toml::from_str(&s)
            .with_context(|| format!("parse spec '{}'", path.display()))?;

        let names = extract_package_names(&raw);
        map.record_tool(&names);
    }

    Ok(map)
}

/// Extract the bare package names from a parsed spec TOML value.
fn extract_package_names(yaml: &toml::Value) -> Vec<String> {
    let Some(pkgs) = yaml.get("packages").and_then(|v| v.as_array()) else {
        return vec![];
    };

    pkgs.iter()
        .filter_map(|v| v.as_str())
        .map(|s| {
            // Strip version constraint: "samtools ==1.19.2" -> "samtools"
            s.split_whitespace().next().unwrap_or(s).to_string()
        })
        .collect()
}

fn walkdir(root: &Path) -> anyhow::Result<impl Iterator<Item = anyhow::Result<std::path::PathBuf>>> {
    let entries = walkdir_inner(root);
    Ok(entries.into_iter())
}

fn walkdir_inner(root: &Path) -> Vec<anyhow::Result<std::path::PathBuf>> {
    let Ok(read) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut results = vec![];
    for entry in read {
        let Ok(e) = entry else { continue };
        let path = e.path();
        if path.is_dir() {
            results.extend(walkdir_inner(&path));
        } else {
            results.push(Ok(path));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_tool_increments_counts() {
        let mut map = PopularityMap::new();
        map.record_tool(&["openssl".into(), "zlib".into()]);
        map.record_tool(&["openssl".into(), "samtools".into()]);
        assert_eq!(map.score("openssl"), 2);
        assert_eq!(map.score("zlib"), 1);
        assert_eq!(map.score("samtools"), 1);
        assert_eq!(map.score("unknown"), 0);
    }

    #[test]
    fn extract_names_strips_version_constraints() {
        let val: toml::Value = toml::from_str(
            "packages = [\"samtools ==1.19.2\", \"openssl\", \"bwa >=0.7\"]",
        )
        .unwrap();
        let names = extract_package_names(&val);
        assert_eq!(names, vec!["samtools", "openssl", "bwa"]);
    }

    #[test]
    fn save_and_load_round_trips() {
        let mut map = PopularityMap::new();
        map.record_tool(&["openssl".into(), "zlib".into()]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("popularity.json");
        map.save(&path).unwrap();

        let loaded = PopularityMap::load(&path).unwrap();
        assert_eq!(loaded.score("openssl"), 1);
        assert_eq!(loaded.score("zlib"), 1);
    }
}
