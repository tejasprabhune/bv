use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;

pub const FASTA_PROTEIN: &str = ">sp|TEST|test_protein Test protein OS=Homo sapiens\nMKTAYIAKQRQISFVKSHFSRQLEDAFQSENEHSFVKKLIENKLEKLNAK\n";
pub const FASTA_NUCLEOTIDE: &str =
    ">test_seq Test nucleotide sequence\nATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGAT\n";
pub const FASTQ: &str = "@read1\nATCGATCGATCGATCG\n+\nIIIIIIIIIIIIIIII\n@read2\nGCTAGCTAGCTAGCTA\n+\nIIIIIIIIIIIIIIII\n";

// Multi-sequence variants for MSA tools (need ≥2 sequences).
pub const FASTA_PROTEIN_MULTI: &str = ">sp|TEST1|prot1\nMKTAYIAKQRQISFVKSHFSRQLEDAFQSENEHSFVKKLIENKLEKLNAK\n\
>sp|TEST2|prot2\nMKTAAIAKQRQISFVKSHFSRQLEDAFQSENEHSFVKKLIENKLEELNAK\n\
>sp|TEST3|prot3\nMKTAYIAKQRQISFVKAHFSRQLEDAFQAENEHSFVKKLIENKLEKLNAK\n";
pub const FASTA_NUCLEOTIDE_MULTI: &str = ">seq1\nATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGAT\n\
>seq2\nATCGATCGATCGATCGATCGATCCATCGATCGATCGATCGATCGATCGAT\n\
>seq3\nATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCGATCCAT\n";

/// Write a `test://` URI to a file in `dest_dir` and return the path.
pub fn materialize(uri: &str, dest_dir: &Path) -> anyhow::Result<PathBuf> {
    let rest = uri
        .strip_prefix("test://")
        .ok_or_else(|| anyhow::anyhow!("expected a test:// URI, got '{}'", uri))?;

    let (content, filename) = match rest {
        "fasta-protein" => (FASTA_PROTEIN, "input.fasta"),
        "fasta-nucleotide" => (FASTA_NUCLEOTIDE, "input.fasta"),
        "fasta-protein-multi" => (FASTA_PROTEIN_MULTI, "input_multi.fasta"),
        "fasta-nucleotide-multi" => (FASTA_NUCLEOTIDE_MULTI, "input_multi.fasta"),
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
