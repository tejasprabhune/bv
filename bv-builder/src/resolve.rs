use std::collections::{HashSet, VecDeque};

use anyhow::{bail, Context, Result};
use reqwest::Client;

use crate::spec::{BuildSpec, PackageSpec, Platform, ResolvedPackage, ResolvedSpec};

/// Resolve a `BuildSpec` to a fully pinned `ResolvedSpec` using the conda
/// repodata from the declared channels.
///
/// Resolution strategy:
/// 1. Download `repodata.json` for each channel + subdir.
/// 2. BFS from the declared packages, resolving each transitive dependency.
/// 3. Return a deterministically sorted `ResolvedSpec`.
pub async fn resolve(spec: &BuildSpec) -> Result<ResolvedSpec> {
    let direct = spec.package_specs()?;
    let subdir = platform_subdir(&spec.platform);

    let client = Client::builder()
        .user_agent("bv-builder/0.1")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build HTTP client")?;

    // Cache repodata to avoid re-downloading per package.
    let mut repodata_cache: std::collections::HashMap<String, RepodataIndex> =
        std::collections::HashMap::new();

    let mut resolved_packages: Vec<ResolvedPackage> = Vec::new();
    let mut resolved_names: HashSet<String> = HashSet::new();

    // (name, is_direct)
    let mut queue: VecDeque<(PackageSpec, bool)> =
        direct.into_iter().map(|p| (p, true)).collect();

    while let Some((pkg_spec, is_direct)) = queue.pop_front() {
        if resolved_names.contains(&pkg_spec.name) || is_virtual_package(&pkg_spec.name) {
            continue;
        }

        let resolved = match resolve_package_cached(
            &client,
            &pkg_spec,
            &spec.channels,
            &subdir,
            &mut repodata_cache,
        )
        .await
        {
            Ok(r) => r,
            Err(e) if !is_direct => {
                eprintln!(
                    "warning: skipping transitive dep '{}': {e}",
                    pkg_spec.name
                );
                resolved_names.insert(pkg_spec.name.clone());
                continue;
            }
            Err(e) => return Err(e),
        };

        for dep_str in &resolved.depends {
            if let Some(dep_spec) = parse_dep_spec(dep_str) {
                if !resolved_names.contains(&dep_spec.name)
                    && !is_virtual_package(&dep_spec.name)
                {
                    queue.push_back((dep_spec, false));
                }
            }
        }

        resolved_names.insert(resolved.name.clone());
        resolved_packages.push(resolved);
    }

    let base = spec.base.clone().or_else(|| {
        Some(match &spec.platform {
            crate::spec::Platform::LinuxAmd64 => "ghcr.io/tejasprabhune/bv-base/debian:12-slim".to_string(),
            crate::spec::Platform::LinuxArm64 => "ghcr.io/tejasprabhune/bv-base/debian:12-slim".to_string(),
        })
    });

    let mut out = ResolvedSpec {
        name: spec.name.clone(),
        version: spec.version.clone(),
        platform: spec.platform.clone(),
        channels: spec.channels.clone(),
        packages: resolved_packages,
        repodata_snapshot: None,
        base,
    };
    out.sort_packages();
    Ok(out)
}

/// Virtual/meta packages that don't have downloadable artifacts.
fn is_virtual_package(name: &str) -> bool {
    name.starts_with("__")
        || matches!(
            name,
            "_libgcc_mutex" | "_openmp_mutex" | "ca-certificates" | "certifi"
        )
}

/// Parse a conda dependency string (e.g. "libgcc-ng >=12.3.0,<13.0a0") into a PackageSpec.
fn parse_dep_spec(dep: &str) -> Option<PackageSpec> {
    let dep = dep.trim();
    // Strip trailing build string markers (e.g. " * nomkl")
    let dep = dep.split(" * ").next().unwrap_or(dep);

    let mut parts = dep.splitn(2, ' ');
    let name = parts.next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let version_spec = parts.next().unwrap_or("*").trim().to_string();
    Some(PackageSpec {
        name,
        version_spec: crate::spec::VersionSpec(version_spec),
    })
}

/// Try each channel in order and return the first match, using a repodata cache.
async fn resolve_package_cached(
    client: &Client,
    pkg_spec: &PackageSpec,
    channels: &[String],
    subdir: &str,
    cache: &mut std::collections::HashMap<String, RepodataIndex>,
) -> Result<ResolvedPackage> {
    for channel in channels {
        for try_subdir in [subdir, "noarch"] {
            let repodata_url = format!("{channel}/{try_subdir}/repodata.json");
            let repodata = if let Some(rd) = cache.get(&repodata_url) {
                rd
            } else {
                let rd: RepodataIndex = match client.get(&repodata_url).send().await {
                    Ok(resp) if resp.status().is_success() => resp
                        .json()
                        .await
                        .with_context(|| format!("parse repodata from {repodata_url}"))?,
                    _ => continue,
                };
                cache.insert(repodata_url.clone(), rd);
                cache.get(&repodata_url).unwrap()
            };

            if let Some(pkg) = find_best_match(repodata, pkg_spec, channel, try_subdir) {
                return Ok(pkg);
            }
        }
    }
    bail!(
        "package '{}' with spec '{}' not found in any channel",
        pkg_spec.name,
        pkg_spec.version_spec
    )
}

/// Find the best (latest) matching package entry in a repodata index.
fn find_best_match(
    repodata: &RepodataIndex,
    pkg_spec: &PackageSpec,
    channel: &str,
    subdir: &str,
) -> Option<ResolvedPackage> {
    let spec_str = pkg_spec.version_spec.0.as_str();

    let mut candidates: Vec<(&str, &RepodataPackageRecord)> = repodata
        .packages_conda
        .iter()
        .chain(repodata.packages.iter())
        .filter(|(_, rec)| {
            rec.name == pkg_spec.name && version_matches(&rec.version, spec_str)
        })
        .map(|(fname, rec)| (fname.as_str(), rec))
        .collect();

    // Sort by version descending, then build descending → pick latest.
    candidates.sort_by(|(_, a), (_, b)| {
        compare_conda_version(&b.version, &a.version)
            .then(b.build_number.cmp(&a.build_number))
    });

    candidates.first().map(|(filename, rec)| {
        let url = format!("{channel}/{subdir}/{filename}");
        ResolvedPackage {
            name: rec.name.clone(),
            version: rec.version.clone(),
            build: rec.build.clone(),
            channel: channel.to_string(),
            url,
            sha256: rec.sha256.clone().unwrap_or_default(),
            filename: filename.to_string(),
            depends: rec.depends.clone(),
        }
    })
}

/// Check if `version` satisfies a conda-style constraint spec.
/// Handles `*`, `==X`, `>=X`, `>X`, `<=X`, `<X`, and comma-separated combinations.
fn version_matches(version: &str, spec: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return true;
    }
    for part in spec.split(',') {
        let part = part.trim();
        if let Some(bound) = part.strip_prefix(">=") {
            if compare_conda_version(version, bound.trim()) == std::cmp::Ordering::Less {
                return false;
            }
        } else if let Some(bound) = part.strip_prefix('>') {
            if compare_conda_version(version, bound.trim()) != std::cmp::Ordering::Greater {
                return false;
            }
        } else if let Some(bound) = part.strip_prefix("<=") {
            if compare_conda_version(version, bound.trim()) == std::cmp::Ordering::Greater {
                return false;
            }
        } else if let Some(bound) = part.strip_prefix('<') {
            if compare_conda_version(version, bound.trim()) != std::cmp::Ordering::Less {
                return false;
            }
        } else if let Some(exact) = part.strip_prefix("==") {
            if version != exact.trim() {
                return false;
            }
        } else if let Some(ne) = part.strip_prefix("!=") {
            if version == ne.trim() {
                return false;
            }
        }
        // Unknown operator: skip conservatively
    }
    true
}

/// Compare two conda version strings using numeric segment ordering.
///
/// Splits on "." and compares each segment numerically.  Segments with a
/// non-numeric suffix (e.g. "0a0", "0b1", "0rc1") are treated as
/// pre-releases and sort before the matching numeric-only segment:
///   "1.22.0a0" < "1.22.0"
/// This matches conda's version ordering so that constraints like
/// `<1.22.0a0` work correctly.
fn compare_conda_version(a: &str, b: &str) -> std::cmp::Ordering {
    let a_segs: Vec<(u64, bool)> = a.split('.').map(version_seg).collect();
    let b_segs: Vec<(u64, bool)> = b.split('.').map(version_seg).collect();
    let len = a_segs.len().max(b_segs.len());
    for i in 0..len {
        let (an, a_pre) = a_segs.get(i).copied().unwrap_or((0, false));
        let (bn, b_pre) = b_segs.get(i).copied().unwrap_or((0, false));
        match an.cmp(&bn) {
            std::cmp::Ordering::Equal => match (a_pre, b_pre) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            },
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Parse one dot-separated version segment into (numeric_value, is_prerelease).
/// "21" → (21, false), "0a0" → (0, true), "0rc1" → (0, true).
fn version_seg(seg: &str) -> (u64, bool) {
    let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
    let is_pre = digits.len() < seg.len();
    (digits.parse().unwrap_or(0), is_pre)
}

fn platform_subdir(platform: &Platform) -> String {
    match platform {
        Platform::LinuxAmd64 => "linux-64".to_string(),
        Platform::LinuxArm64 => "linux-aarch64".to_string(),
    }
}

// Repodata index structures

#[derive(Debug, serde::Deserialize)]
struct RepodataIndex {
    #[serde(default)]
    pub packages: std::collections::HashMap<String, RepodataPackageRecord>,
    #[serde(default, rename = "packages.conda")]
    pub packages_conda: std::collections::HashMap<String, RepodataPackageRecord>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RepodataPackageRecord {
    pub name: String,
    pub version: String,
    pub build: String,
    #[serde(default)]
    pub build_number: u32,
    pub sha256: Option<String>,
    #[serde(default)]
    pub depends: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_star() {
        assert!(version_matches("1.19.2", "*"));
        assert!(version_matches("1.19.2", ""));
    }

    #[test]
    fn version_matches_exact() {
        assert!(version_matches("1.19.2", "==1.19.2"));
        assert!(!version_matches("1.18.0", "==1.19.2"));
    }

    #[test]
    fn version_matches_gte() {
        assert!(version_matches("1.21", ">=1.21"));
        assert!(version_matches("1.21.0", ">=1.21"));
        assert!(!version_matches("1.9", ">=1.21"));
        assert!(!version_matches("1.20.5", ">=1.21"));
    }

    #[test]
    fn version_matches_range() {
        assert!(version_matches("1.21.0", ">=1.21,<1.22.0a0"));
        assert!(!version_matches("1.9", ">=1.21,<1.22.0a0"));
        assert!(!version_matches("1.22.0", ">=1.21,<1.22.0a0"));
    }

    #[test]
    fn compare_numeric_version_order() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_conda_version("1.21", "1.9"), Greater);
        assert_eq!(compare_conda_version("1.9", "1.21"), Less);
        assert_eq!(compare_conda_version("1.21.0", "1.21"), Equal);
        assert_eq!(compare_conda_version("2.0.0", "1.99.99"), Greater);
    }

    #[test]
    fn compare_prerelease_sorts_before_release() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_conda_version("1.22.0a0", "1.22.0"), Less);
        assert_eq!(compare_conda_version("1.22.0", "1.22.0a0"), Greater);
        assert_eq!(compare_conda_version("1.21.0", "1.22.0a0"), Less);
    }
}
