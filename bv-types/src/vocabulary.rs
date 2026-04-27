use std::collections::HashMap;
use std::sync::OnceLock;

use crate::types::{TypeDef, TypeId};

#[derive(serde::Deserialize)]
struct VocabFile {
    types: HashMap<TypeId, TypeDef>,
}

static VOCAB: OnceLock<HashMap<TypeId, TypeDef>> = OnceLock::new();

pub fn vocabulary() -> &'static HashMap<TypeId, TypeDef> {
    VOCAB.get_or_init(|| {
        let src = include_str!("../types.toml");
        let v: VocabFile = toml::from_str(src).expect("built-in types.toml is invalid");
        v.types
    })
}

pub fn lookup(id: &str) -> Option<&'static TypeDef> {
    vocabulary().get(id)
}

pub fn known_type_ids() -> impl Iterator<Item = &'static str> {
    vocabulary().keys().map(String::as_str)
}

/// Levenshtein distance, capped at `limit` for early exit.
fn edit_distance(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m.abs_diff(n) > limit {
        return limit + 1;
    }
    let mut row: Vec<usize> = (0..=n).collect();
    for i in 1..=m {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=n {
            let old = row[j];
            row[j] = if a[i - 1] == b[j - 1] {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j - 1])
            };
            prev = old;
        }
    }
    row[n]
}

/// Returns the closest known type id to `unknown`, if within edit distance 3.
pub fn suggest(unknown: &str) -> Option<String> {
    known_type_ids()
        .map(|id| (id, edit_distance(unknown, id, 3)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(id, _)| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types_load() {
        let v = vocabulary();
        assert!(v.contains_key("fasta"));
        assert!(v.contains_key("blast_db"));
        assert!(v.contains_key("dir"));
    }

    #[test]
    fn suggest_close_typo() {
        // "fasq" is distance 1 from "fastq"
        assert_eq!(suggest("fasq").as_deref(), Some("fastq"));
    }

    #[test]
    fn suggest_far_returns_none() {
        // "protien_fasta" is too far from any known type to suggest
        assert_eq!(suggest("protien_fasta"), None);
    }

    #[test]
    fn suggest_near_miss() {
        assert!(suggest("fase").is_some());
    }
}
