use std::path::Path;
use std::process::{Command, Stdio};

use bv_core::error::{BvError, Result};
use bv_runtime::OciRef;

/// Pull an OCI image as a SIF file. Returns the path to the SIF file.
pub fn pull_as_sif(image: &OciRef, sif_path: &Path, apptainer_bin: &str) -> Result<()> {
    if let Some(parent) = sif_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| BvError::RuntimeError(format!("failed to create SIF cache dir: {e}")))?;
    }

    let uri = registry_uri(image);

    let status = Command::new(apptainer_bin)
        .args([
            "pull",
            "--force",
            sif_path.to_string_lossy().as_ref(),
            &uri,
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| BvError::RuntimeNotAvailable {
            runtime: "apptainer".into(),
            reason: format!("could not execute `{apptainer_bin}`: {e}"),
        })?;

    if !status.success() {
        return Err(BvError::RuntimeError(format!(
            "failed to pull SIF for '{uri}' (exit {})",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Build the `docker://` URI that Apptainer uses to pull from an OCI registry.
///
/// Apptainer's `oci://` scheme refers to a local OCI image layout on disk;
/// remote registry pulls (ghcr.io, quay.io, docker.io, ...) all go through
/// `docker://`. The docker.io registry is implicit, so we omit it.
pub fn registry_uri(image: &OciRef) -> String {
    let mut uri = if image.registry == "docker.io" {
        format!("docker://{}", image.repository)
    } else {
        format!("docker://{}/{}", image.registry, image.repository)
    };
    if let Some(digest) = &image.digest {
        uri.push('@');
        uri.push_str(digest);
    } else if let Some(tag) = &image.tag {
        uri.push(':');
        uri.push_str(tag);
    }
    uri
}
