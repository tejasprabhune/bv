use bv_core::cache::CacheLayout;
use bv_core::error::Result;
use bv_core::project::BvToml;
use bv_runtime::{
    ContainerRuntime, DockerRuntime, GpuProfile, ImageDigest, ImageMetadata, Mount, OciRef,
    ProgressReporter, RunOutcome, RunSpec, RuntimeInfo,
};
use bv_runtime_apptainer::{ApptainerRuntime, is_available as apptainer_available};

/// A runtime that can be either Docker or Apptainer, chosen at startup.
#[derive(Clone)]
pub enum AnyRuntime {
    Docker(DockerRuntime),
    Apptainer(ApptainerRuntime),
}

impl ContainerRuntime for AnyRuntime {
    fn name(&self) -> &str {
        match self {
            Self::Docker(r) => r.name(),
            Self::Apptainer(r) => r.name(),
        }
    }

    fn health_check(&self) -> Result<RuntimeInfo> {
        match self {
            Self::Docker(r) => r.health_check(),
            Self::Apptainer(r) => r.health_check(),
        }
    }

    fn pull(&self, image: &OciRef, progress: &dyn ProgressReporter) -> Result<ImageDigest> {
        match self {
            Self::Docker(r) => r.pull(image, progress),
            Self::Apptainer(r) => r.pull(image, progress),
        }
    }

    fn run(&self, spec: &RunSpec) -> Result<RunOutcome> {
        match self {
            Self::Docker(r) => r.run(spec),
            Self::Apptainer(r) => r.run(spec),
        }
    }

    fn inspect(&self, digest: &ImageDigest) -> Result<ImageMetadata> {
        match self {
            Self::Docker(r) => r.inspect(digest),
            Self::Apptainer(r) => r.inspect(digest),
        }
    }

    fn is_locally_available(&self, image_ref: &str, digest: &str) -> bool {
        match self {
            Self::Docker(r) => r.is_locally_available(image_ref, digest),
            Self::Apptainer(r) => r.is_locally_available(image_ref, digest),
        }
    }

    fn gpu_args(&self, profile: &GpuProfile) -> Vec<String> {
        match self {
            Self::Docker(r) => r.gpu_args(profile),
            Self::Apptainer(r) => r.gpu_args(profile),
        }
    }

    fn mount_args(&self, mounts: &[Mount]) -> Vec<String> {
        match self {
            Self::Docker(r) => r.mount_args(mounts),
            Self::Apptainer(r) => r.mount_args(mounts),
        }
    }
}

/// Select a runtime from (in priority order): explicit flag, bv.toml, auto-detect.
pub fn resolve_runtime(
    backend_flag: Option<&str>,
    bv_toml: Option<&BvToml>,
) -> anyhow::Result<AnyRuntime> {
    let backend = backend_flag
        .or_else(|| bv_toml.and_then(|t| t.runtime.backend.as_deref()))
        .unwrap_or("auto");

    build_runtime(backend)
}

fn build_runtime(backend: &str) -> anyhow::Result<AnyRuntime> {
    match backend {
        "docker" => Ok(AnyRuntime::Docker(DockerRuntime)),
        "apptainer" | "singularity" => {
            let cache = CacheLayout::new();
            Ok(AnyRuntime::Apptainer(ApptainerRuntime::new(
                cache.sif_dir(),
            )))
        }
        "auto" => {
            if DockerRuntime.health_check().is_ok() {
                return Ok(AnyRuntime::Docker(DockerRuntime));
            }
            if apptainer_available() {
                let cache = CacheLayout::new();
                return Ok(AnyRuntime::Apptainer(ApptainerRuntime::new(
                    cache.sif_dir(),
                )));
            }
            anyhow::bail!(
                "no container runtime found\n  \
                 Install Docker (https://docs.docker.com/get-docker/) or \
                 Apptainer (https://apptainer.org/docs/admin/main/installation.html)"
            )
        }
        other => anyhow::bail!(
            "unknown backend '{}'; use 'docker', 'apptainer', or 'auto'",
            other
        ),
    }
}
