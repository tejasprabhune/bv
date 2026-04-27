use std::path::Path;

use anyhow::Context;
use bv_core::manifest::IoSpec;

/// Verify that an output file or directory exists and passes a minimal
/// format check for its declared type.
pub fn check_output(spec: &IoSpec, path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!(
            "output '{}' does not exist at {}",
            spec.name,
            path.display()
        );
    }

    let type_id = spec.r#type.base_id();
    check_format(type_id, path).with_context(|| {
        format!(
            "output '{}' failed format check for type '{}'",
            spec.name, type_id
        )
    })
}

fn check_format(type_id: &str, path: &Path) -> anyhow::Result<()> {
    match type_id {
        "dir" | "blast_db" | "mmseqs_db" => {
            if !path.is_dir() {
                anyhow::bail!("expected a directory");
            }
        }
        "fasta" => check_fasta(path)?,
        "fastq" => check_fastq(path)?,
        "bam" => check_bam(path)?,
        "hmm_profile" => check_hmm(path)?,
        "tabular" | "blast_tab" | "sam" | "vcf" | "mmseqs_output" | "hmmer_output" => {
            check_tabular(path)?
        }
        _ => {
            if !path.exists() {
                anyhow::bail!("file does not exist");
            }
        }
    }
    Ok(())
}

fn check_fasta(path: &Path) -> anyhow::Result<()> {
    let first = first_byte(path)?;
    if first != b'>' {
        anyhow::bail!("FASTA files must start with '>'");
    }
    Ok(())
}

fn check_fastq(path: &Path) -> anyhow::Result<()> {
    let first = first_byte(path)?;
    if first != b'@' {
        anyhow::bail!("FASTQ files must start with '@'");
    }
    Ok(())
}

fn check_bam(path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).context("could not read BAM file")?;
    if bytes.len() < 4 || &bytes[..4] != b"BAM\x01" {
        anyhow::bail!("BAM magic bytes not found");
    }
    Ok(())
}

fn check_hmm(path: &Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path).context("could not read HMM profile")?;
    if !content.contains("HMMER3") {
        anyhow::bail!("HMMER3 header not found in HMM profile");
    }
    Ok(())
}

fn check_tabular(path: &Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path).context("could not read tabular file")?;
    if content.is_empty() {
        anyhow::bail!("tabular output is empty");
    }
    let data_lines: Vec<_> = content.lines().filter(|l| !l.starts_with('#')).collect();
    if data_lines.is_empty() {
        return Ok(());
    }
    let col_count = data_lines[0].split('\t').count();
    if col_count < 2 {
        anyhow::bail!("tabular output has fewer than 2 columns");
    }
    Ok(())
}

fn first_byte(path: &Path) -> anyhow::Result<u8> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).context("could not open file")?;
    let mut buf = [0u8; 1];
    f.read_exact(&mut buf).context("file is empty")?;
    Ok(buf[0])
}
