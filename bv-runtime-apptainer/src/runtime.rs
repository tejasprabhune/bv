use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use bv_core::error::{BvError, Result};
use bv_runtime::{
    ContainerRuntime, GpuProfile, ImageDigest, ImageMetadata, Mount, OciRef, ProgressReporter,
    RunOutcome, RunSpec, RuntimeInfo,
};

use crate::cache::{file_sha256, sif_path};
use crate::gpu::nv_args;
use crate::image::pull_as_sif;
use crate::mount::bind_args;

#[derive(Debug, Clone)]
pub struct ApptainerRuntime {
    sif_dir: PathBuf,
    bin: String,
}

impl ApptainerRuntime {
    pub fn new(sif_dir: PathBuf) -> Self {
        let bin = if Command::new("apptainer")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            "apptainer"
        } else {
            "singularity"
        };
        Self {
            sif_dir,
            bin: bin.to_string(),
        }
    }

    fn sif_path_for(&self, reference: &str) -> PathBuf {
        sif_path(&self.sif_dir, reference)
    }
}

impl ContainerRuntime for ApptainerRuntime {
    fn name(&self) -> &str {
        "apptainer"
    }

    fn health_check(&self) -> Result<RuntimeInfo> {
        let out = Command::new(&self.bin)
            .arg("version")
            .output()
            .map_err(|e| BvError::RuntimeNotAvailable {
                runtime: "apptainer".into(),
                reason: format!("could not execute `{}`: {e}", self.bin),
            })?;

        if !out.status.success() {
            return Err(BvError::RuntimeNotAvailable {
                runtime: "apptainer".into(),
                reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }

        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let mut extra = HashMap::new();
        extra.insert("binary".into(), self.bin.clone());

        Ok(RuntimeInfo {
            name: self.bin.clone(),
            version,
            extra,
        })
    }

    fn pull(&self, image: &OciRef, progress: &dyn ProgressReporter) -> Result<ImageDigest> {
        let reference = image.to_string();
        let sif = self.sif_path_for(&reference);

        if sif.exists() {
            progress.finish(&format!("SIF already cached for {reference}"));
            let digest = file_sha256(&sif)
                .map_err(|e| BvError::RuntimeError(format!("failed to hash cached SIF: {e}")))?;
            return Ok(ImageDigest(digest));
        }

        progress.update(&format!("Pulling {reference} as SIF"), None, None);
        pull_as_sif(image, &sif, &self.bin)?;

        let digest = file_sha256(&sif)
            .map_err(|e| BvError::RuntimeError(format!("failed to hash SIF: {e}")))?;
        progress.finish(&format!("Pulled {reference}"));
        Ok(ImageDigest(digest))
    }

    fn run(&self, spec: &RunSpec) -> Result<RunOutcome> {
        let start = Instant::now();
        let reference = spec.image.to_string();
        let sif = self.sif_path_for(&reference);

        if !sif.exists() {
            return Err(BvError::RuntimeError(format!(
                "SIF not found for '{}'; run `bv sync` to pull it",
                reference
            )));
        }

        let mut cmd = Command::new(&self.bin);
        cmd.arg("run");

        if let Some(wd) = &spec.working_dir {
            cmd.args(["--pwd", &wd.to_string_lossy()]);
        }

        for arg in bind_args(&spec.mounts) {
            cmd.arg(arg);
        }

        for arg in nv_args(&spec.gpu) {
            cmd.arg(arg);
        }

        for (k, v) in &spec.env {
            cmd.arg("--env").arg(format!("{k}={v}"));
        }

        cmd.arg(sif.to_string_lossy().as_ref());

        for arg in &spec.command {
            cmd.arg(arg);
        }

        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let status = cmd
            .status()
            .map_err(|e| BvError::RuntimeError(format!("apptainer run failed: {e}")))?;

        Ok(RunOutcome {
            exit_code: status.code().unwrap_or(-1),
            duration: start.elapsed(),
        })
    }

    fn inspect(&self, digest: &ImageDigest) -> Result<ImageMetadata> {
        let sif = self
            .sif_dir
            .join(format!("{}.sif", digest.0.replace(':', "_")));
        let size_bytes = if sif.exists() {
            std::fs::metadata(&sif).ok().map(|m| m.len())
        } else {
            None
        };
        Ok(ImageMetadata {
            digest: digest.clone(),
            size_bytes,
            labels: HashMap::new(),
        })
    }

    fn is_locally_available(&self, image_ref: &str, _digest: &str) -> bool {
        self.sif_path_for(image_ref).exists()
    }

    fn gpu_args(&self, profile: &GpuProfile) -> Vec<String> {
        nv_args(profile)
    }

    fn mount_args(&self, mounts: &[Mount]) -> Vec<String> {
        bind_args(mounts)
    }
}

/// Return true if any Apptainer/Singularity binary is present on PATH.
pub fn is_available() -> bool {
    for bin in &["apptainer", "singularity"] {
        if Command::new(bin)
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}
