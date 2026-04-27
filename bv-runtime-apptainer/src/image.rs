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

    let oci_uri = oci_uri(image);

    let status = Command::new(apptainer_bin)
        .args([
            "pull",
            "--force",
            sif_path.to_string_lossy().as_ref(),
            &oci_uri,
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
            "failed to pull SIF for '{oci_uri}' (exit {})",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Build the `oci://` URI that Apptainer understands.
pub fn oci_uri(image: &OciRef) -> String {
    let mut uri = format!("oci://{}/{}", image.registry, image.repository);
    if image.registry == "docker.io" {
        uri = format!("docker://{}", image.repository);
    }
    if let Some(digest) = &image.digest {
        uri.push('@');
        uri.push_str(digest);
    } else if let Some(tag) = &image.tag {
        uri.push(':');
        uri.push_str(tag);
    }
    uri
}
