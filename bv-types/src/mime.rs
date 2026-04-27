use crate::vocabulary;

/// Returns the MIME hint for `type_id`, walking up the hierarchy until one is found.
pub fn mime_hint(type_id: &str) -> Option<String> {
    let vocab = vocabulary::vocabulary();
    let mut current = type_id;
    loop {
        match vocab.get(current) {
            None => return None,
            Some(def) => {
                if let Some(mime) = &def.mime {
                    return Some(mime.clone());
                }
                match def.parent.as_deref() {
                    None => return None,
                    Some(p) => current = p,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fasta_mime() {
        assert_eq!(
            mime_hint("fasta").as_deref(),
            Some("application/x-fasta")
        );
    }

    #[test]
    fn blast_tab_inherits_tabular_mime() {
        assert_eq!(
            mime_hint("blast_tab").as_deref(),
            Some("text/tab-separated-values")
        );
    }

    #[test]
    fn blast_db_no_mime() {
        assert_eq!(mime_hint("blast_db"), None);
    }

    #[test]
    fn unknown_no_mime() {
        assert_eq!(mime_hint("no_such_type"), None);
    }
}
