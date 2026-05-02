use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use bv_core::error::{BvError, Result};
use bv_runtime::{
    ContainerRuntime, GpuProfile, ImageDigest, ImageMetadata, ImageRef, LayerSpec, Mount, OciRef,
    ProgressReporter, RunOutcome, RunSpec, RuntimeInfo,
};

use crate::blob_cache::{LayerIndex, layer_index_path, supports_oci_native};
use crate::cache::{file_sha256, sif_path_for_digest};
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

    fn sif_for_digest(&self, digest: &str) -> PathBuf {
        sif_path_for_digest(&self.sif_dir, digest)
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

        // Fast path: caller already knows the digest (e.g. `bv sync` from a
        // lockfile) and the SIF is on disk under its canonical key.
        // Re-hash the cached file to catch on-disk tampering or corruption
        // before silently trusting it.
        if let Some(known) = &image.digest {
            let canonical = self.sif_for_digest(known);
            if canonical.exists() {
                let actual = file_sha256(&canonical).map_err(|e| {
                    BvError::RuntimeError(format!(
                        "failed to hash cached SIF {}: {e}",
                        canonical.display()
                    ))
                })?;
                if &actual == known {
                    // Clear the spinner; the outer "Added" line is the success summary.
                    progress.finish("");
                    return Ok(ImageDigest(known.clone()));
                }
                // Mismatch: drop the bad file and fall through to a fresh pull.
                progress.update(
                    &format!(
                        "cached SIF {} sha256 {} does not match expected {}; re-pulling",
                        canonical.display(),
                        actual,
                        known
                    ),
                    None,
                    None,
                );
                let _ = std::fs::remove_file(&canonical);
            }
        }

        // Pull to a temp path keyed by the registry reference, then rename
        // into <digest>.sif so subsequent lookups by digest find it.
        std::fs::create_dir_all(&self.sif_dir)
            .map_err(|e| BvError::RuntimeError(format!("failed to create SIF cache dir: {e}")))?;
        let staging = self
            .sif_dir
            .join(format!(".pull-{}.sif", std::process::id()));

        progress.update(&format!("Pulling {reference} as SIF"), None, None);
        {
            let _paused = progress.pause();
            pull_as_sif(image, &staging, &self.bin)?;
        }

        let digest = file_sha256(&staging)
            .map_err(|e| BvError::RuntimeError(format!("failed to hash SIF: {e}")))?;
        let canonical = self.sif_for_digest(&digest);
        if canonical.exists() {
            // Another concurrent pull beat us to it.
            let _ = std::fs::remove_file(&staging);
        } else if let Err(e) = std::fs::rename(&staging, &canonical) {
            // Cross-device or other rename failure: fall back to copy + remove.
            std::fs::copy(&staging, &canonical).map_err(|e2| {
                BvError::RuntimeError(format!(
                    "failed to place SIF in cache (rename: {e}; copy: {e2})"
                ))
            })?;
            let _ = std::fs::remove_file(&staging);
        }
        progress.finish("");
        Ok(ImageDigest(digest))
    }

    fn run(&self, spec: &RunSpec) -> Result<RunOutcome> {
        let start = Instant::now();
        let reference = spec.image.to_string();
        let digest = spec.image.digest.as_deref().ok_or_else(|| {
            BvError::RuntimeError(format!(
                "apptainer run requires a pinned digest for '{reference}'"
            ))
        })?;
        let sif = self.sif_for_digest(digest);

        if !sif.exists() {
            return Err(BvError::RuntimeError(format!(
                "SIF not found for '{}'; run `bv sync` to pull it",
                reference
            )));
        }

        let mut cmd = Command::new(&self.bin);
        cmd.arg("run");

        // Isolate the container from the host:
        //   --cleanenv: do not inherit the user's environment (HOME, PATH,
        //               SSH_*, AWS_* secrets, etc.). Manifest-declared env
        //               vars are still passed via explicit `--env` flags
        //               below, which `--cleanenv` does not affect.
        //   --no-home : do not bind-mount the host $HOME into the container.
        // Apptainer/Singularity already runs as the calling uid:gid by
        // default, so an explicit `--userns` is not required here (Fix #9).
        cmd.arg("--cleanenv").arg("--no-home");

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

        if spec.capture_output {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = cmd
                .output()
                .map_err(|e| BvError::RuntimeError(format!("apptainer run failed: {e}")))?;
            return Ok(RunOutcome {
                exit_code: output.status.code().unwrap_or(-1),
                duration: start.elapsed(),
                stdout: output.stdout,
                stderr: output.stderr,
            });
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
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn inspect(&self, digest: &ImageDigest) -> Result<ImageMetadata> {
        let sif = self.sif_for_digest(&digest.0);
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

    fn is_locally_available(&self, _image_ref: &str, digest: &str) -> bool {
        self.sif_for_digest(digest).exists()
    }

    fn gpu_args(&self, profile: &GpuProfile) -> Vec<String> {
        nv_args(profile)
    }

    fn mount_args(&self, mounts: &[Mount]) -> Vec<String> {
        bind_args(mounts)
    }

    /// Check whether all `layers` are already covered by a cached SIF.
    ///
    /// Consults the layer index at `<sif_dir>/layer-index.json`. If every
    /// layer digest in the list maps to a SIF that still exists on disk,
    /// the pull can be skipped entirely; this is the primary dedup mechanism
    /// for the Apptainer backend when two tools share conda package layers.
    fn ensure_layers(
        &self,
        layers: &[LayerSpec],
        progress: &dyn ProgressReporter,
    ) -> bv_core::error::Result<()> {
        if layers.is_empty() {
            return Ok(());
        }

        let index_path = layer_index_path(&self.sif_dir);
        let index = LayerIndex::load_or_create(&index_path).unwrap_or_default();

        let all_cached = layers.iter().all(|l| {
            index
                .sif_for_layer(&l.digest)
                .map(|sif_digest| self.sif_for_digest(sif_digest).exists())
                .unwrap_or(false)
        });

        if all_cached {
            progress.update(
                &format!("All {} layers cached (reusing existing SIF)", layers.len()),
                None,
                None,
            );
        }

        Ok(())
    }

    /// Pull a factored OCI image as a SIF, then record the layer→SIF mapping
    /// in the layer index so future `ensure_layers` calls can short-circuit.
    ///
    /// If the Apptainer binary version predates OCI-native mode (< 1.2.0), a
    /// warning is printed but the pull proceeds via the standard `docker://`
    /// URI; Apptainer will download and convert the whole image regardless of
    /// layer structure.
    fn assemble_image(
        &self,
        image: &OciRef,
        layers: &[LayerSpec],
        progress: &dyn ProgressReporter,
    ) -> bv_core::error::Result<ImageRef> {
        if !layers.is_empty() && !supports_oci_native(&self.bin) {
            progress.update(
                "Warning: Apptainer < 1.2 does not support OCI-native mode; \
                 layer-granularity dedup is unavailable, pulling full image",
                None,
                None,
            );
        }

        let digest = self.pull(image, progress)?;

        // Record each layer → this SIF in the index so future ensure_layers
        // calls can skip the pull when the same layers appear in another tool.
        if !layers.is_empty() {
            let index_path = layer_index_path(&self.sif_dir);
            if let Ok(mut index) = LayerIndex::load_or_create(&index_path) {
                for layer in layers {
                    index.record(&layer.digest, &digest.0);
                }
                let _ = index.save(&index_path);
            }
        }

        Ok(ImageRef {
            reference: image.to_string(),
            digest: digest.0,
        })
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
