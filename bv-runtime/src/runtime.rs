use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use bv_core::error::Result;
use bv_core::manifest::GpuSpec;

// OciRef

#[derive(Debug, Clone)]
pub struct OciRef {
    pub registry: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl OciRef {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        s.parse()
    }

    /// Return the string Docker expects for `docker pull` / `docker run`.
    /// For docker.io images the registry prefix is stripped so that Docker Hub
    /// resolves references correctly across all Docker versions.
    pub fn docker_arg(&self) -> String {
        if self.registry == "docker.io" {
            let mut s = self.repository.clone();
            if let Some(tag) = &self.tag {
                s.push(':');
                s.push_str(tag);
            }
            if let Some(digest) = &self.digest {
                s.push('@');
                s.push_str(digest);
            }
            s
        } else {
            self.to_string()
        }
    }
}

impl fmt::Display for OciRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.registry, self.repository)?;
        if let Some(tag) = &self.tag {
            write!(f, ":{tag}")?;
        }
        if let Some(digest) = &self.digest {
            write!(f, "@{digest}")?;
        }
        Ok(())
    }
}

impl FromStr for OciRef {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let s = raw
            .strip_prefix("docker://")
            .or_else(|| raw.strip_prefix("oci://"))
            .unwrap_or(raw);

        let (image_part, digest) = if let Some((img, d)) = s.split_once('@') {
            (img, Some(d.to_string()))
        } else {
            (s, None)
        };

        // Tag detection rules:
        // - If there is no '/', the input is a single image name with an
        //   optional tag (e.g. `foo:1.0`).
        // - Otherwise, only the segment after the last '/' may carry a tag.
        //   This avoids treating a registry port (`localhost:5000/foo`) as a tag.
        let (name_part, tag) = if !image_part.contains('/') {
            if let Some(pos) = image_part.rfind(':') {
                (&image_part[..pos], Some(image_part[pos + 1..].to_string()))
            } else {
                (image_part, None)
            }
        } else {
            let last_slash = image_part.rfind('/').unwrap();
            let last_segment = &image_part[last_slash + 1..];
            if let Some(rel_colon) = last_segment.find(':') {
                let split = last_slash + 1 + rel_colon;
                (
                    &image_part[..split],
                    Some(image_part[split + 1..].to_string()),
                )
            } else {
                (image_part, None)
            }
        };

        let (registry, repository) = split_registry(name_part);

        Ok(OciRef {
            registry,
            repository,
            tag,
            digest,
        })
    }
}

fn split_registry(name: &str) -> (String, String) {
    if let Some(slash_pos) = name.find('/') {
        let potential_registry = &name[..slash_pos];
        if potential_registry.contains('.')
            || potential_registry.contains(':')
            || potential_registry == "localhost"
        {
            return (
                potential_registry.to_string(),
                name[slash_pos + 1..].to_string(),
            );
        }
    }
    ("docker.io".to_string(), name.to_string())
}

// Supporting types

#[derive(Debug, Clone)]
pub struct ImageDigest(pub String);

impl fmt::Display for ImageDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ImageMetadata {
    pub digest: ImageDigest,
    pub size_bytes: Option<u64>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    pub name: String,
    pub version: String,
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GpuProfile {
    pub spec: Option<GpuSpec>,
}

#[derive(Debug, Clone)]
pub struct RunSpec {
    /// OCI reference of the image to run (may carry a pinned digest).
    pub image: OciRef,
    pub command: Vec<String>,
    pub env: HashMap<String, String>,
    pub mounts: Vec<Mount>,
    pub gpu: GpuProfile,
    pub working_dir: Option<PathBuf>,
    /// When true, capture stdout+stderr into `RunOutcome` instead of inheriting
    /// to the host. Used by `bv conformance` to inspect probe output without
    /// flooding the user's terminal.
    #[doc(hidden)]
    pub capture_output: bool,
}

#[derive(Debug, Default)]
pub struct RunOutcome {
    pub exit_code: i32,
    pub duration: Duration,
    /// Captured stdout when `RunSpec.capture_output` is true; empty otherwise.
    pub stdout: Vec<u8>,
    /// Captured stderr when `RunSpec.capture_output` is true; empty otherwise.
    pub stderr: Vec<u8>,
}

// ProgressReporter

pub trait ProgressReporter: Send + Sync {
    fn update(&self, message: &str, current: Option<u64>, total: Option<u64>);
    fn finish(&self, message: &str);

    /// Hide our own spinner/bars while a child process draws to the terminal.
    /// Implementations should clear their lines until the returned guard is dropped.
    /// The default is a no-op for reporters that don't draw to a TTY.
    fn pause(&self) -> Box<dyn PauseGuard + '_> {
        Box::new(NoopPauseGuard)
    }
}

pub trait PauseGuard {}

pub struct NoopPauseGuard;
impl PauseGuard for NoopPauseGuard {}

pub struct NoopProgress;

impl ProgressReporter for NoopProgress {
    fn update(&self, _: &str, _: Option<u64>, _: Option<u64>) {}
    fn finish(&self, _: &str) {}
}

// ContainerRuntime trait

/// Descriptor for a single OCI layer that needs to be available locally.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub digest: String,
    pub size: u64,
    pub media_type: String,
    /// Source URL for pulling the layer blob when not already cached.
    pub blob_url: Option<String>,
}

/// A locally-available image identified by an OCI reference + digest.
#[derive(Debug, Clone)]
pub struct ImageRef {
    pub reference: String,
    pub digest: String,
}

pub trait ContainerRuntime {
    fn name(&self) -> &str;
    fn health_check(&self) -> Result<RuntimeInfo>;
    fn pull(&self, image: &OciRef, progress: &dyn ProgressReporter) -> Result<ImageDigest>;
    fn run(&self, spec: &RunSpec) -> Result<RunOutcome>;
    fn inspect(&self, digest: &ImageDigest) -> Result<ImageMetadata>;
    /// Check whether `image_ref@digest` is already in the local Docker cache.
    fn is_locally_available(&self, _image_ref: &str, digest: &str) -> bool {
        self.inspect(&ImageDigest(digest.to_string())).is_ok()
    }
    fn gpu_args(&self, profile: &GpuProfile) -> Vec<String>;
    fn mount_args(&self, mounts: &[Mount]) -> Vec<String>;

    /// Pull only the specified layers, deduplicating against the local cache.
    ///
    /// For `factored_oci` tools, callers pass the per-package layer list and
    /// the runtime ensures each layer blob is present locally before
    /// `assemble_image` is called.
    ///
    /// The default implementation is a no-op (runtimes that don't support
    /// factored pulls fall back to `pull`).
    fn ensure_layers(
        &self,
        _layers: &[LayerSpec],
        _progress: &dyn ProgressReporter,
    ) -> Result<()> {
        Ok(())
    }

    /// Assemble a runnable image from a manifest whose layers are all locally
    /// available (guaranteed by a preceding `ensure_layers` call).
    ///
    /// Returns a locally-addressable `ImageRef`. The default implementation
    /// falls back to `pull` for runtimes that don't support layer assembly.
    fn assemble_image(
        &self,
        image: &OciRef,
        _layers: &[LayerSpec],
        progress: &dyn ProgressReporter,
    ) -> Result<ImageRef> {
        let digest = self.pull(image, progress)?;
        Ok(ImageRef {
            reference: image.to_string(),
            digest: digest.0,
        })
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_image() {
        let r: OciRef = "ubuntu:22.04".parse().unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "ubuntu");
        assert_eq!(r.tag.as_deref(), Some("22.04"));
        assert!(r.digest.is_none());
    }

    #[test]
    fn parse_with_registry() {
        let r: OciRef = "ghcr.io/biocontainers/bwa:0.7.17".parse().unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "biocontainers/bwa");
        assert_eq!(r.tag.as_deref(), Some("0.7.17"));
    }

    #[test]
    fn parse_with_digest() {
        let r: OciRef = "ubuntu@sha256:abc123".parse().unwrap();
        assert_eq!(r.digest.as_deref(), Some("sha256:abc123"));
        assert!(r.tag.is_none());
    }

    #[test]
    fn parse_docker_scheme() {
        let r: OciRef = "docker://biocontainers/bwa:0.7.17".parse().unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "biocontainers/bwa");
    }

    #[test]
    fn docker_arg_strips_docker_io() {
        let r: OciRef = "ncbi/blast:2.14.0".parse().unwrap();
        assert_eq!(r.docker_arg(), "ncbi/blast:2.14.0");

        let mut r2: OciRef = "ncbi/blast:2.14.0".parse().unwrap();
        r2.tag = None;
        r2.digest = Some("sha256:abc123".into());
        assert_eq!(r2.docker_arg(), "ncbi/blast@sha256:abc123");
    }

    #[test]
    fn docker_arg_keeps_external_registry() {
        let r: OciRef = "quay.io/biocontainers/blast:2.15.0".parse().unwrap();
        assert_eq!(r.docker_arg(), "quay.io/biocontainers/blast:2.15.0");
    }

    #[test]
    fn parse_localhost_port_registry_with_repo() {
        let r: OciRef = "localhost:5000/foo/bar".parse().unwrap();
        assert_eq!(r.registry, "localhost:5000");
        assert_eq!(r.repository, "foo/bar");
        assert!(r.tag.is_none());
        assert!(r.digest.is_none());
    }

    #[test]
    fn parse_localhost_port_registry_with_repo_and_tag() {
        let r: OciRef = "localhost:5000/foo/bar:1.0".parse().unwrap();
        assert_eq!(r.registry, "localhost:5000");
        assert_eq!(r.repository, "foo/bar");
        assert_eq!(r.tag.as_deref(), Some("1.0"));
    }

    #[test]
    fn parse_single_name() {
        let r: OciRef = "foo".parse().unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "foo");
        assert!(r.tag.is_none());
    }

    #[test]
    fn parse_single_name_with_tag() {
        let r: OciRef = "foo:1.0".parse().unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "foo");
        assert_eq!(r.tag.as_deref(), Some("1.0"));
    }

    #[test]
    fn parse_two_segments_no_registry() {
        let r: OciRef = "foo/bar".parse().unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "foo/bar");
        assert!(r.tag.is_none());
    }

    #[test]
    fn parse_quay_with_tag() {
        let r: OciRef = "quay.io/biocontainers/blast:2.14.0".parse().unwrap();
        assert_eq!(r.registry, "quay.io");
        assert_eq!(r.repository, "biocontainers/blast");
        assert_eq!(r.tag.as_deref(), Some("2.14.0"));
    }

    #[test]
    fn parse_ghcr_with_digest() {
        let r: OciRef = "ghcr.io/owner/repo@sha256:abc".parse().unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "owner/repo");
        assert!(r.tag.is_none());
        assert_eq!(r.digest.as_deref(), Some("sha256:abc"));
    }
}
