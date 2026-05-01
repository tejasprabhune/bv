use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::Path;

pub struct OwnedImages {
    pub digests: BTreeSet<String>,
    pub references: BTreeSet<String>,
}

impl OwnedImages {
    pub fn load(path: &Path) -> Self {
        let mut digests = BTreeSet::new();
        let mut references = BTreeSet::new();
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let mut parts = line.splitn(2, '\t');
                let reference = parts.next().unwrap_or("").trim();
                let digest = parts.next().unwrap_or("").trim();
                if !reference.is_empty() && !digest.is_empty() {
                    references.insert(reference.to_string());
                    digests.insert(digest.to_string());
                }
            }
        }
        Self { digests, references }
    }

    pub fn is_empty(&self) -> bool {
        self.digests.is_empty()
    }
}

/// Append a `reference\tdigest` line to the owned-images file.
/// Idempotent: no-ops if the digest is already recorded.
pub fn record(path: &Path, reference: &str, digest: &str) -> io::Result<()> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        if content.lines().any(|line| {
            line.splitn(2, '\t').nth(1).unwrap_or("").trim() == digest
        }) {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}\t{}", reference, digest)
}

/// Remove the line whose digest column matches `digest`.
pub fn remove_by_digest(path: &Path, digest: &str) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)?;
    let new_content: String = content
        .lines()
        .filter(|line| line.splitn(2, '\t').nth(1).unwrap_or("").trim() != digest)
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(path, new_content)
}
