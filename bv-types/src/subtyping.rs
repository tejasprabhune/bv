use crate::vocabulary;

/// Returns true if `child` is the same type as `parent` or a descendant of it.
pub fn is_subtype_of(child: &str, parent: &str) -> bool {
    if child == parent {
        return true;
    }
    let vocab = vocabulary::vocabulary();
    let mut current = child;
    loop {
        match vocab.get(current).and_then(|d| d.parent.as_deref()) {
            None => return false,
            Some(p) if p == parent => return true,
            Some(p) => current = p,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fasta_is_subtype_of_file() {
        assert!(is_subtype_of("fasta", "file"));
    }

    #[test]
    fn blast_tab_is_subtype_of_tabular_and_file() {
        assert!(is_subtype_of("blast_tab", "tabular"));
        assert!(is_subtype_of("blast_tab", "file"));
    }

    #[test]
    fn file_is_not_subtype_of_fasta() {
        assert!(!is_subtype_of("file", "fasta"));
    }

    #[test]
    fn identity() {
        assert!(is_subtype_of("fasta", "fasta"));
    }

    #[test]
    fn unknown_type_is_not_subtype() {
        assert!(!is_subtype_of("unknown_type", "file"));
    }
}
