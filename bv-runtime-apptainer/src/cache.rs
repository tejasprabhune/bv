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

/// Canonical SIF cache path for a known digest.
///
/// All callsites (`pull`, `run`, `is_locally_available`) agree on this form,
/// so that an image pulled by `bv sync` is found by `bv run`/`bv exec` even
/// though those commands construct different `OciRef`s (with/without tag).
/// `digest` is expected in the form `sha256:<hex>`; the colon is sanitized.
pub fn sif_path_for_digest(sif_dir: &Path, digest: &str) -> PathBuf {
    let safe: String = digest
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    sif_dir.join(format!("{safe}.sif"))
}

/// Compute the SHA-256 of a file and return `sha256:<hex>`.
///
/// Streams the file in 64 KiB chunks so multi-GB SIFs do not need to be held
/// in memory.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use std::fs::File;
    use std::io::{BufReader, Read};

    use sha2::{Digest, Sha256};

    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize();
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_sha256_streams_known_value() {
        // Known sha256 of "hello world" is
        // b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9.
        let dir = std::env::temp_dir().join(format!("bv_cache_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello world").unwrap();
        }
        let got = file_sha256(&path).unwrap();
        assert_eq!(
            got,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn file_sha256_handles_chunk_boundary() {
        // Write more than one 64 KiB chunk so we exercise the loop.
        let dir = std::env::temp_dir().join(format!("bv_cache_test_chunk_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            let chunk = vec![0xABu8; 70 * 1024];
            f.write_all(&chunk).unwrap();
        }
        let got = file_sha256(&path).unwrap();
        assert!(got.starts_with("sha256:"));
        assert_eq!(got.len(), "sha256:".len() + 64);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
