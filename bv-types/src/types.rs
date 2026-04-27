use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub type TypeId = String;

/// Whether a type node is a plain file or a directory layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    File,
    Directory,
}

/// One entry in the type vocabulary (from `types.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDef {
    pub description: String,
    /// Set on root types (`file`, `dir`, `blast_db`, ...); mutually exclusive with `parent`.
    pub kind: Option<TypeKind>,
    /// Parent type id; mutually exclusive with `kind`.
    pub parent: Option<TypeId>,
    #[serde(default)]
    pub mime: Option<String>,
    /// Named parameters this type accepts (e.g. `["alphabet"]` for `fasta`).
    #[serde(default)]
    pub parameters: Vec<String>,
    /// Required file globs for composite directory types.
    #[serde(default)]
    pub required_files: Vec<String>,
    /// Semantic column layout for tabular subtypes.
    #[serde(default)]
    pub columns: Option<String>,
}

/// A resolved type reference: base id plus optional parameter values.
/// Parses `"fasta"` and `"fasta[protein]"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub id: TypeId,
    pub params: Vec<String>,
}

impl TypeRef {
    pub fn base_id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.params.is_empty() {
            write!(f, "{}", self.id)
        } else {
            write!(f, "{}[{}]", self.id, self.params.join(","))
        }
    }
}

impl FromStr for TypeRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((id, rest)) = s.split_once('[') {
            let params_str = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("unclosed '[' in type ref '{s}'"))?;
            let params = params_str
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            Ok(TypeRef {
                id: id.to_string(),
                params,
            })
        } else {
            Ok(TypeRef {
                id: s.to_string(),
                params: vec![],
            })
        }
    }
}

impl Serialize for TypeRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TypeRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// How many values an I/O port accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// Exactly one value; required.
    #[default]
    One,
    /// One or more values.
    Many,
    /// Zero or one value; optional.
    Optional,
}

impl fmt::Display for Cardinality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cardinality::One => write!(f, "one"),
            Cardinality::Many => write!(f, "many"),
            Cardinality::Optional => write!(f, "optional"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typeref_no_params() {
        let r: TypeRef = "fasta".parse().unwrap();
        assert_eq!(r.id, "fasta");
        assert!(r.params.is_empty());
        assert_eq!(r.to_string(), "fasta");
    }

    #[test]
    fn typeref_with_params() {
        let r: TypeRef = "fasta[protein]".parse().unwrap();
        assert_eq!(r.id, "fasta");
        assert_eq!(r.params, vec!["protein"]);
        assert_eq!(r.to_string(), "fasta[protein]");
    }

    #[test]
    fn typeref_multi_params() {
        let r: TypeRef = "msa[stockholm,nucleotide]".parse().unwrap();
        assert_eq!(r.id, "msa");
        assert_eq!(r.params, vec!["stockholm", "nucleotide"]);
    }

    #[test]
    fn typeref_unclosed_bracket() {
        assert!("fasta[protein".parse::<TypeRef>().is_err());
    }
}
