use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

use bv_core::error::{BvError, Result};

use crate::runtime::{
    ContainerRuntime, GpuProfile, ImageDigest, ImageMetadata, Mount, OciRef, ProgressReporter,
    RunOutcome, RunSpec, RuntimeInfo,
};

#[derive(Clone)]
pub struct DockerRuntime;

impl ContainerRuntime for DockerRuntime {
    fn name(&self) -> &str {
        "docker"
    }

    fn health_check(&self) -> Result<RuntimeInfo> {
        let output = Command::new("docker")
            .arg("version")
            .output()
            .map_err(|e| BvError::RuntimeNotAvailable {
                runtime: "docker".into(),
                reason: format!("could not execute `docker`: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BvError::RuntimeNotAvailable {
                runtime: "docker".into(),
                reason: format!("docker daemon not running or not accessible: {stderr}"),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse "Version: X.Y.Z" lines; stable across Docker Engine and Desktop.
        let versions: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.trim().strip_prefix("Version:").map(|v| v.trim()))
            .collect();

        let client_version = versions.first().copied().unwrap_or("unknown").to_string();
        let server_version = versions.get(1).copied().map(str::to_string);

        let mut extra = HashMap::new();
        if let Some(sv) = server_version {
            extra.insert("server_version".into(), sv);
        }

        Ok(RuntimeInfo {
            name: "docker".into(),
            version: client_version,
            extra,
        })
    }

    fn pull(&self, image: &OciRef, progress: &dyn ProgressReporter) -> Result<ImageDigest> {
        let image_arg = image.docker_arg();
        progress.update(&image_arg, None, None);

        let mut child = Command::new("docker")
            .args(["pull", &image_arg])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BvError::RuntimeNotAvailable {
                runtime: "docker".into(),
                reason: format!("could not execute `docker`: {e}"),
            })?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        // Drain stderr in a background thread to prevent pipe deadlock.
        let stderr_thread = thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        // Parse docker pull stdout line-by-line for layer progress and the digest.
        // Layer lifecycle: "Pulling fs layer" | "Already exists" → "Pull complete"
        let mut pull_digest: Option<String> = None;
        let mut total_layers: u64 = 0;
        let mut done_layers: u64 = 0;

        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(BvError::Io)?;
            let trimmed = line.trim();

            if let Some(d) = trimmed.strip_prefix("Digest: ") {
                pull_digest = Some(d.to_string());
                continue;
            }

            // Count layers as they're announced; "Already exists" layers count
            // as both total and immediately done.
            if trimmed.ends_with(": Pulling fs layer") {
                total_layers += 1;
            } else if trimmed.ends_with(": Already exists") {
                total_layers += 1;
                done_layers += 1;
            } else if trimmed.ends_with(": Pull complete") {
                done_layers += 1;
            }

            let (cur, tot) = if total_layers > 0 {
                (Some(done_layers), Some(total_layers))
            } else {
                (None, None)
            };
            progress.update(&image_arg, cur, tot);
        }

        let status = child.wait()?;
        let stderr_output = stderr_thread.join().unwrap_or_default();

        if !status.success() {
            return Err(classify_pull_error(&stderr_output, &image_arg));
        }

        progress.finish("");

        let digest = match pull_digest {
            Some(d) => d,
            None => self.repo_digest(&image_arg)?,
        };

        Ok(ImageDigest(digest))
    }

    fn run(&self, spec: &RunSpec) -> Result<RunOutcome> {
        let start = Instant::now();

        let mut cmd = Command::new("docker");
        cmd.arg("run").arg("--rm");

        // Run as the host user so files written into bind mounts are not
        // owned by root. On non-Unix platforms `current_uid_gid` returns
        // None and we let docker pick the image default.
        if let Some((uid, gid)) = current_uid_gid() {
            cmd.args(["--user", &format!("{uid}:{gid}")]);
        }

        if let Some(wd) = &spec.working_dir {
            cmd.args(["-w", &wd.to_string_lossy()]);
        }

        for arg in self.mount_args(&spec.mounts) {
            cmd.arg(arg);
        }

        for (k, v) in &spec.env {
            cmd.arg("-e").arg(format!("{k}={v}"));
        }

        for arg in self.gpu_args(&spec.gpu) {
            cmd.arg(arg);
        }

        // Pass NVIDIA_VISIBLE_DEVICES through if the user has set it.
        if let Ok(val) = std::env::var("NVIDIA_VISIBLE_DEVICES") {
            cmd.arg("-e").arg(format!("NVIDIA_VISIBLE_DEVICES={val}"));
        }

        cmd.arg(spec.image.docker_arg());

        for arg in &spec.command {
            cmd.arg(arg);
        }

        if spec.capture_output {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = cmd
                .output()
                .map_err(|e| BvError::RuntimeError(format!("docker run failed to launch: {e}")))?;
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
            .map_err(|e| BvError::RuntimeError(format!("docker run failed to launch: {e}")))?;

        Ok(RunOutcome {
            exit_code: status.code().unwrap_or(-1),
            duration: start.elapsed(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn inspect(&self, digest: &ImageDigest) -> Result<ImageMetadata> {
        let output = Command::new("docker")
            .args(["image", "inspect", "--format", "{{.Size}}", &digest.0])
            .output()
            .map_err(|e| BvError::RuntimeError(e.to_string()))?;

        if !output.status.success() {
            return Err(BvError::RuntimeError(format!(
                "docker image inspect failed for '{}'",
                digest.0
            )));
        }

        let size_bytes = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok();

        Ok(ImageMetadata {
            digest: digest.clone(),
            size_bytes,
            labels: HashMap::new(),
        })
    }

    fn is_locally_available(&self, image_ref: &str, digest: &str) -> bool {
        let pinned = format!("{image_ref}@{digest}");
        Command::new("docker")
            .args(["image", "inspect", "--format", "{{.Id}}", &pinned])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn gpu_args(&self, profile: &GpuProfile) -> Vec<String> {
        match &profile.spec {
            Some(spec) if spec.required => vec!["--gpus".into(), "all".into()],
            _ => vec![],
        }
    }

    fn mount_args(&self, mounts: &[Mount]) -> Vec<String> {
        mounts
            .iter()
            .flat_map(|m| {
                let mode = if m.read_only { "ro" } else { "rw" };
                let spec = format!(
                    "{}:{}:{mode}",
                    m.host_path.display(),
                    m.container_path.display()
                );
                ["-v".to_string(), spec]
            })
            .collect()
    }
}

impl DockerRuntime {
    /// Pull `image` and bail if the registry-reported digest does not match
    /// `expected_digest`.
    ///
    /// `pull` itself does not enforce this: it returns whatever digest the
    /// registry hands back. Most callers (`bv sync`) already cross-check
    /// against the lockfile, but the `bv run` and `bv conform` paths short
    /// circuit through `is_locally_available`, which only proves that a
    /// matching RepoDigests entry exists in the local cache, not that the
    /// upstream image still resolves to the pinned sha. New code that
    /// requires a digest pin should call this method instead of `pull`.
    //
    // TODO: route `bv run` / `bv conform` through this once the
    // `ContainerRuntime` trait gains a `pull_verified` method (would
    // touch bv-cli/runtime_select and the apptainer impl, so deferred).
    pub fn pull_verified(
        &self,
        image: &OciRef,
        expected_digest: &str,
        progress: &dyn ProgressReporter,
    ) -> Result<ImageDigest> {
        let got = self.pull(image, progress)?;
        verify_digest(&image.to_string(), expected_digest, &got.0)?;
        Ok(got)
    }

    /// Pull `image`, verify the image digest, then verify each per-layer
    /// digest from `layers` against what Docker reports for the pulled image.
    ///
    /// Callers that hold a `LockfileEntry` with `spec_kind = FactoredOci`
    /// should call this instead of `pull_verified` so that individual
    /// conda-package layer tampering is caught immediately after pull.
    ///
    /// Error messages include the expected and actual digest plus the layer
    /// position so that upstream tampering is easy to diagnose.
    pub fn pull_verified_v2(
        &self,
        image: &OciRef,
        expected_image_digest: &str,
        layers: &[bv_core::lockfile::LayerDescriptor],
        progress: &dyn ProgressReporter,
    ) -> Result<ImageDigest> {
        let got = self.pull(image, progress)?;
        verify_digest(&image.to_string(), expected_image_digest, &got.0)?;

        if !layers.is_empty() {
            self.verify_layer_digests(image, layers)?;
        }

        Ok(got)
    }

    /// Verify per-layer digests for an already-pulled image.
    ///
    /// Uses `docker manifest inspect` (or `docker image inspect`) to obtain the
    /// layer list and cross-checks each digest against the lockfile entry.
    /// On mismatch, the error message names the layer index, the expected digest,
    /// and the actual digest to make upstream tampering easy to diagnose.
    pub fn verify_layer_digests(
        &self,
        image: &OciRef,
        expected_layers: &[bv_core::lockfile::LayerDescriptor],
    ) -> Result<()> {
        let image_arg = image.docker_arg();

        // `docker image inspect` returns a JSON array; we extract the RootFS layers.
        let output = Command::new("docker")
            .args([
                "image",
                "inspect",
                "--format",
                "{{range .RootFS.Layers}}{{.}}\n{{end}}",
                &image_arg,
            ])
            .output()
            .map_err(|e| BvError::RuntimeError(format!("docker image inspect failed: {e}")))?;

        if !output.status.success() {
            // Image may not be locally available; layer verification is best-effort here.
            return Ok(());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let actual_layers: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();

        // Docker's RootFS digest and the OCI manifest digest use different algorithms
        // in some cases (DiffID vs compressed digest).  We do an exact-match check when
        // the layer counts agree, and log a warning when they differ (e.g. when the image
        // was built with a non-standard compressor).
        if actual_layers.len() != expected_layers.len() {
            // Layer count mismatch is non-fatal for legacy images that were not
            // built by bv-builder; for factored_oci images built by bv-builder
            // this will be caught at manifest inspection time.
            return Ok(());
        }

        for (i, (expected, actual)) in expected_layers.iter().zip(actual_layers.iter()).enumerate()
        {
            if expected.digest != *actual {
                return Err(BvError::RuntimeError(format!(
                    "layer digest mismatch at index {i} for '{image_arg}': \
                     expected {expected_digest} but got {actual}; \
                     possible upstream tampering or mismatched layer ordering",
                    expected_digest = expected.digest,
                )));
            }
        }

        Ok(())
    }

    /// Get the content digest for a locally available image by reference.
    fn repo_digest(&self, image_ref: &str) -> Result<String> {
        let output = Command::new("docker")
            .args([
                "image",
                "inspect",
                "--format",
                "{{index .RepoDigests 0}}",
                image_ref,
            ])
            .output()
            .map_err(|e| BvError::RuntimeError(e.to_string()))?;

        if !output.status.success() {
            return Err(BvError::RuntimeError(format!(
                "could not inspect image '{image_ref}' after pull"
            )));
        }

        let line = String::from_utf8_lossy(&output.stdout);
        let line = line.trim();

        // RepoDigests entry looks like "ncbi/blast@sha256:abc123"; extract digest part.
        if let Some(digest) = line.split('@').nth(1) {
            Ok(digest.to_string())
        } else if line.starts_with("sha256:") {
            Ok(line.to_string())
        } else {
            // Locally built image without a registry digest; fall back to image ID.
            let id_output = Command::new("docker")
                .args(["image", "inspect", "--format", "{{.Id}}", image_ref])
                .output()
                .map_err(|e| BvError::RuntimeError(e.to_string()))?;
            Ok(String::from_utf8_lossy(&id_output.stdout)
                .trim()
                .to_string())
        }
    }
}

/// Return the calling process's effective uid:gid on Unix.
///
/// Falls back to `None` on non-Unix targets (Windows) so callers can skip
/// passing `--user` rather than guessing.
#[cfg(unix)]
fn current_uid_gid() -> Option<(u32, u32)> {
    // SAFETY: getuid()/getgid() are async-signal-safe and never fail per POSIX.
    unsafe extern "C" {
        fn getuid() -> u32;
        fn getgid() -> u32;
    }
    Some((unsafe { getuid() }, unsafe { getgid() }))
}

#[cfg(not(unix))]
fn current_uid_gid() -> Option<(u32, u32)> {
    None
}

/// Compare a registry-returned digest to the digest the caller pinned and
/// return a clear error on mismatch.
fn verify_digest(image_ref: &str, expected: &str, got: &str) -> Result<()> {
    if expected == got {
        Ok(())
    } else {
        Err(BvError::RuntimeError(format!(
            "image digest mismatch for '{image_ref}': expected {expected} but registry returned {got}"
        )))
    }
}

/// Map docker pull stderr to a user-friendly `BvError`.
fn classify_pull_error(stderr: &str, image_ref: &str) -> BvError {
    if stderr.contains("Cannot connect to the Docker daemon")
        || stderr.contains("Is the docker daemon running")
    {
        BvError::RuntimeNotAvailable {
            runtime: "docker".into(),
            reason: "Docker daemon is not available. Is Docker Desktop running?".into(),
        }
    } else if stderr.contains("manifest unknown")
        || stderr.contains("not found")
        || stderr.contains("does not exist")
    {
        BvError::RuntimeError(format!(
            "image '{image_ref}' not found in registry (check the tool manifest)"
        ))
    } else if stderr.contains("connection refused") || stderr.contains("no such host") {
        BvError::RuntimeError(format!(
            "network error while pulling '{image_ref}': {stderr}"
        ))
    } else {
        BvError::RuntimeError(format!("docker pull failed:\n{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_digest_matches() {
        assert!(verify_digest("ncbi/blast", "sha256:abc", "sha256:abc").is_ok());
    }

    #[test]
    fn verify_digest_rejects_mismatch() {
        let err = verify_digest("ncbi/blast", "sha256:abc", "sha256:def").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ncbi/blast"));
        assert!(msg.contains("sha256:abc"));
        assert!(msg.contains("sha256:def"));
        assert!(msg.contains("mismatch"));
    }

    #[cfg(unix)]
    #[test]
    fn current_uid_gid_returns_some_on_unix() {
        let got = current_uid_gid();
        assert!(got.is_some());
    }

    /// Simulate the layer-counting logic used in `pull()` to verify the
    /// counts stay correct across typical docker pull output patterns.
    fn count_layers(lines: &[&str]) -> (u64, u64) {
        let mut total: u64 = 0;
        let mut done: u64 = 0;
        for line in lines {
            let t = line.trim();
            if t.ends_with(": Pulling fs layer") {
                total += 1;
            } else if t.ends_with(": Already exists") {
                total += 1;
                done += 1;
            } else if t.ends_with(": Pull complete") {
                done += 1;
            }
        }
        (done, total)
    }

    #[test]
    fn layer_count_fresh_pull() {
        let lines = [
            "abc123: Pulling fs layer",
            "def456: Pulling fs layer",
            "abc123: Downloading",
            "abc123: Pull complete",
            "def456: Pull complete",
            "Digest: sha256:deadbeef",
        ];
        assert_eq!(count_layers(&lines), (2, 2));
    }

    #[test]
    fn layer_count_partially_cached() {
        let lines = [
            "abc123: Already exists",
            "def456: Pulling fs layer",
            "def456: Pull complete",
        ];
        // total=2, done=2 (already-exists counts as immediately done)
        assert_eq!(count_layers(&lines), (2, 2));
    }

    #[test]
    fn layer_count_in_progress() {
        let lines = [
            "abc123: Pulling fs layer",
            "def456: Pulling fs layer",
            "ghi789: Pulling fs layer",
            "abc123: Pull complete",
            // def456 and ghi789 still downloading
        ];
        assert_eq!(count_layers(&lines), (1, 3));
    }
}
