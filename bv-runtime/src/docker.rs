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
        progress.update(&format!("Pulling {image_arg}"), None, None);

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

        // Parse docker pull stdout line-by-line for progress and the digest.
        let mut pull_digest: Option<String> = None;
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(BvError::Io)?;
            let trimmed = line.trim();
            // "Digest: sha256:abc123"
            if let Some(d) = trimmed.strip_prefix("Digest: ") {
                pull_digest = Some(d.to_string());
            }
            progress.update(trimmed, None, None);
        }

        let status = child.wait()?;
        let stderr_output = stderr_thread.join().unwrap_or_default();

        if !status.success() {
            return Err(classify_pull_error(&stderr_output, &image_arg));
        }

        progress.finish(""); // outer "Added X" line is the summary; avoid redundant URL spam

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

        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let status = cmd
            .status()
            .map_err(|e| BvError::RuntimeError(format!("docker run failed to launch: {e}")))?;

        Ok(RunOutcome {
            exit_code: status.code().unwrap_or(-1),
            duration: start.elapsed(),
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
