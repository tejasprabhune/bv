use std::path::{Path, PathBuf};

/// Derive a stable SIF filename from an OCI reference string.
/// E.g. `ncbi/blast:2.15.0` -> `ncbi_blast_2.15.0.sif`
pub fn sif_path(sif_dir: &Path, reference: &str) -> PathBuf {
    let safe: String = reference
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sif_dir.join(format!("{safe}.sif"))
}

/// Compute the SHA-256 of a file and return `sha256:<hex>`.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}
