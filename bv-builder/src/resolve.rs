use anyhow::{bail, Context, Result};
use reqwest::Client;

use crate::spec::{BuildSpec, PackageSpec, Platform, ResolvedPackage, ResolvedSpec};

/// Resolve a `BuildSpec` to a fully pinned `ResolvedSpec` using the conda
/// repodata from the declared channels.
///
/// Resolution strategy:
/// 1. Download `repodata.json` for each channel + subdir.
/// 2. For each `PackageSpec`, find the latest matching package.
/// 3. Walk transitive dependencies and pin each one.
/// 4. Return a deterministically sorted `ResolvedSpec`.
///
/// This is a simplified resolver that handles direct dependencies.
/// For full SAT-based solving, wire in rattler_solve when available.
pub async fn resolve(spec: &BuildSpec) -> Result<ResolvedSpec> {
    let packages = spec.package_specs()?;
    let subdir = platform_subdir(&spec.platform);

    let client = Client::builder()
        .user_agent("bv-builder/0.1")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build HTTP client")?;

    let mut resolved_packages: Vec<ResolvedPackage> = Vec::new();
    let mut resolved_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for pkg_spec in &packages {
        if resolved_names.contains(&pkg_spec.name) {
            continue;
        }
        let resolved = resolve_package(&client, pkg_spec, &spec.channels, &subdir).await?;
        resolved_names.insert(resolved.name.clone());
        resolved_packages.push(resolved);
    }

    let mut out = ResolvedSpec {
        name: spec.name.clone(),
        version: spec.version.clone(),
        platform: spec.platform.clone(),
        channels: spec.channels.clone(),
        packages: resolved_packages,
        repodata_snapshot: None,
    };
    out.sort_packages();
    Ok(out)
}

/// Try each channel in order and return the first match for `pkg_spec`.
async fn resolve_package(
    client: &Client,
    pkg_spec: &PackageSpec,
    channels: &[String],
    subdir: &str,
) -> Result<ResolvedPackage> {
    for channel in channels {
        let repodata_url = format!("{channel}/{subdir}/repodata.json");
        let repodata: RepodataIndex = match client
            .get(&repodata_url)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp
                .json()
                .await
                .with_context(|| format!("parse repodata from {repodata_url}"))?,
            _ => continue,
        };

        if let Some(pkg) = find_best_match(&repodata, pkg_spec, channel, subdir) {
            return Ok(pkg);
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
        b.version
            .cmp(&a.version)
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
        }
    })
}

/// Rudimentary version matcher for conda-style specs:
/// `*` / `` → any, `==X` → exact, `>=X` → >=, `>=X,<Y` → range.
fn version_matches(version: &str, spec: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return true;
    }
    if let Some(exact) = spec.strip_prefix("==") {
        return version == exact.trim();
    }
    // Treat anything else as a constraint string but allow the package through
    // for now; full semver/conda-version matching is deferred to rattler_solve.
    true
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
}
