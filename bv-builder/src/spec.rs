use serde::{Deserialize, Serialize};

// Platform

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    #[serde(rename = "linux/amd64")]
    LinuxAmd64,
    #[serde(rename = "linux/arm64")]
    LinuxArm64,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::LinuxAmd64 => write!(f, "linux/amd64"),
            Platform::LinuxArm64 => write!(f, "linux/arm64"),
        }
    }
}

// VersionSpec

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionSpec(pub String);

impl std::fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// PackageSpec

/// One package requirement in a build spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSpec {
    pub name: String,
    pub version_spec: VersionSpec,
}

impl PackageSpec {
    /// Parse `samtools ==1.19.2` or `samtools` into a PackageSpec.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();
        if let Some((name, spec)) = s.split_once(' ') {
            Ok(Self {
                name: name.trim().to_string(),
                version_spec: VersionSpec(spec.trim().to_string()),
            })
        } else {
            Ok(Self {
                name: s.to_string(),
                version_spec: VersionSpec("*".to_string()),
            })
        }
    }
}

// EntrypointSpec

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrypointSpec {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

// BuildSpec

/// Input spec for `bv-builder build`, mirroring apko's declarative format
/// but adapted for conda packages.
///
/// Spec YAML example:
/// ```yaml
/// name: samtools
/// version: 1.19.2
/// channels:
///   - https://conda.anaconda.org/bioconda
///   - https://conda.anaconda.org/conda-forge
/// packages:
///   - samtools ==1.19.2
/// entrypoint:
///   command: /opt/conda/envs/env/bin/samtools
/// platform: linux/amd64
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSpec {
    pub name: String,
    pub version: String,
    pub channels: Vec<String>,
    /// Package requirements in `name ==version` form, or just `name`.
    pub packages: Vec<String>,
    pub entrypoint: EntrypointSpec,
    pub platform: Platform,
    /// Optional base OCI image to pull layers from before the conda layers.
    /// Defaults to `debian:12-slim` when not specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

impl BuildSpec {
    pub fn package_specs(&self) -> anyhow::Result<Vec<PackageSpec>> {
        self.packages.iter().map(|s| PackageSpec::parse(s)).collect()
    }
}

// ResolvedPackage

/// One fully-pinned conda package produced by `resolve()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub build: String,
    pub channel: String,
    /// Direct download URL for the package archive.
    pub url: String,
    /// sha256 of the archive bytes (hex, no prefix).
    pub sha256: String,
    /// File name: `<name>-<version>-<build>.conda` or `.tar.bz2`.
    pub filename: String,
    /// Runtime dependencies declared by this package (used during transitive resolution).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
}

// ResolvedSpec

/// Output of `resolve()`: every package exactly pinned.
///
/// Two `resolve()` calls with the same `BuildSpec` pointing at the same
/// frozen repodata snapshots must produce byte-identical `ResolvedSpec`
/// instances (deterministic sort order by name+version+build).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSpec {
    pub name: String,
    pub version: String,
    pub platform: Platform,
    pub channels: Vec<String>,
    pub packages: Vec<ResolvedPackage>,
    /// Optional: path/URL to the repodata snapshot used during resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repodata_snapshot: Option<String>,
    /// Base OCI image reference to include as the first layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

impl ResolvedSpec {
    /// Sort packages deterministically: name → version → build.
    pub fn sort_packages(&mut self) {
        self.packages.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.version.cmp(&b.version))
                .then(a.build.cmp(&b.build))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_spec_parse_with_version() {
        let ps = PackageSpec::parse("samtools ==1.19.2").unwrap();
        assert_eq!(ps.name, "samtools");
        assert_eq!(ps.version_spec.0, "==1.19.2");
    }

    #[test]
    fn package_spec_parse_bare_name() {
        let ps = PackageSpec::parse("openssl").unwrap();
        assert_eq!(ps.name, "openssl");
        assert_eq!(ps.version_spec.0, "*");
    }

    #[test]
    fn resolved_spec_sort_is_deterministic() {
        let mut spec = ResolvedSpec {
            name: "test".into(),
            version: "1.0".into(),
            platform: Platform::LinuxAmd64,
            channels: vec![],
            packages: vec![
                ResolvedPackage {
                    name: "zlib".into(),
                    version: "1.3.1".into(),
                    build: "h0_0".into(),
                    channel: "conda-forge".into(),
                    url: "https://example.com/zlib.conda".into(),
                    sha256: "abc".into(),
                    filename: "zlib-1.3.1-h0_0.conda".into(),
                    depends: vec![],
                },
                ResolvedPackage {
                    name: "openssl".into(),
                    version: "3.2.1".into(),
                    build: "h0_0".into(),
                    channel: "conda-forge".into(),
                    url: "https://example.com/openssl.conda".into(),
                    sha256: "def".into(),
                    filename: "openssl-3.2.1-h0_0.conda".into(),
                    depends: vec![],
                },
            ],
            repodata_snapshot: None,
            base: None,
        };
        spec.sort_packages();
        assert_eq!(spec.packages[0].name, "openssl");
        assert_eq!(spec.packages[1].name, "zlib");
    }
}
