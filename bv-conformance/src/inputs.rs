use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;

pub const FASTA_PROTEIN: &str = ">sp|TEST|test_protein Test protein OS=Homo sapiens\nMKTAYIAKQRQISFVKSHFSRQLEDAFQSENEHSFVKKLIENKLEKLNAK\n";
pub const FASTA_NUCLEOTIDE: &str =
    ">test_seq Test nucleotide sequence\nATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGAT\n";
pub const FASTQ: &str = "@read1\nATCGATCGATCGATCG\n+\nIIIIIIIIIIIIIIII\n@read2\nGCTAGCTAGCTAGCTA\n+\nIIIIIIIIIIIIIIII\n";

/// Write a `test://` URI to a file in `dest_dir` and return the path.
pub fn materialize(uri: &str, dest_dir: &Path) -> anyhow::Result<PathBuf> {
    let rest = uri
        .strip_prefix("test://")
        .ok_or_else(|| anyhow::anyhow!("expected a test:// URI, got '{}'", uri))?;

    let (content, filename) = match rest {
        "fasta-protein" => (FASTA_PROTEIN, "input.fasta"),
        "fasta-nucleotide" => (FASTA_NUCLEOTIDE, "input.fasta"),
        "fastq" => (FASTQ, "input.fastq"),
        other => anyhow::bail!(
            "unknown test input '{}'; add it to bv-conformance/src/inputs.rs",
            other
        ),
    };

    let path = dest_dir.join(filename);
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write test input to {}", path.display()))?;
    Ok(path)
}

/// Materialize all test inputs into `dest_dir`.
pub fn materialize_all(
    inputs: &HashMap<String, String>,
    dest_dir: &Path,
) -> anyhow::Result<HashMap<String, PathBuf>> {
    inputs
        .iter()
        .map(|(port_name, uri)| {
            let path = materialize(uri, dest_dir)?;
            Ok((port_name.clone(), path))
        })
        .collect()
}
