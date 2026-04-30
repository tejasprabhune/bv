use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};

pub type BinaryIndex = BTreeMap<String, String>;

// SpecKind

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    /// Single squashed image as pulled from biocontainers / a legacy registry.
    #[default]
    LegacyImage,
    /// Factored OCI image where each conda package is its own layer.
    FactoredOci,
}

// CondaPackagePin

/// Exact conda package that a layer was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CondaPackagePin {
    pub name: String,
    pub version: String,
    pub build: String,
    pub channel: String,
    /// sha256 of the .conda / .tar.bz2 archive (hex, no prefix).
    pub sha256: String,
}

// LayerDescriptor

/// One OCI layer entry in a lockfile tool record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerDescriptor {
    /// Content digest of the compressed layer blob (e.g. `sha256:abc...`).
    pub digest: String,
    pub size: u64,
    pub media_type: String,
    /// Present only for `factored_oci` layers that correspond to a single conda package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conda_package: Option<CondaPackagePin>,
}

impl LayerDescriptor {
    pub fn new_zstd(digest: impl Into<String>, size: u64) -> Self {
        Self {
            digest: digest.into(),
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
            conda_package: None,
        }
    }

    pub fn new_gzip(digest: impl Into<String>, size: u64) -> Self {
        Self {
            digest: digest.into(),
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
            conda_package: None,
        }
    }
}

// ReferenceDataPin

/// Per-dataset pin stored inside a lockfile entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceDataPin {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

// LockfileEntry

/// One resolved tool entry in `bv.lock`.
///
/// Stability fields used by `bv lock --check` to detect drift:
/// `tool_id`, `version`, `image_digest`, `manifest_sha256`,
/// and the `digest` of every layer for `factored_oci` entries.
/// Timestamps and sizes are informational only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockfileEntry {
    pub tool_id: String,
    /// Version requirement as declared in `bv.toml` (e.g. `=2.14.0`, `^2`, or `*`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub declared_version_req: String,
    /// Resolved semver (e.g. `2.14.0`).
    pub version: String,
    /// How the image was built; drives the pull path and layer verification strategy.
    #[serde(default, skip_serializing_if = "SpecKind::is_legacy")]
    pub spec_kind: SpecKind,
    /// Canonical OCI reference from the manifest (e.g. `ncbi/blast:2.14.0`).
    pub image_reference: String,
    /// Content digest of the pulled image (e.g. `sha256:abc123...`).
    pub image_digest: String,
    /// SHA-256 of the manifest TOML at resolve time; used for drift detection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_sha256: String,
    pub image_size_bytes: Option<u64>,
    /// Per-layer descriptors (ordered as they appear in the OCI manifest).
    /// Empty for `legacy_image` entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerDescriptor>,
    pub resolved_at: DateTime<Utc>,
    #[serde(default)]
    pub reference_data_pins: BTreeMap<String, ReferenceDataPin>,
    /// Binary names this tool contributes to the binary index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binaries: Vec<String>,
}

impl SpecKind {
    pub fn is_legacy(&self) -> bool {
        matches!(self, SpecKind::LegacyImage)
    }
}

impl LockfileEntry {
    /// True when two entries represent the same resolved state.
    /// Ignores timestamps, sizes, and declared_version_req.
    /// For `factored_oci` entries, all layer digests must also match.
    pub fn is_equivalent(&self, other: &Self) -> bool {
        if self.tool_id != other.tool_id
            || self.version != other.version
            || self.image_digest != other.image_digest
        {
            return false;
        }
        if !self.manifest_sha256.is_empty()
            && !other.manifest_sha256.is_empty()
            && self.manifest_sha256 != other.manifest_sha256
        {
            return false;
        }
        if self.layers.len() != other.layers.len() {
            return false;
        }
        self.layers
            .iter()
            .zip(other.layers.iter())
            .all(|(a, b)| a.digest == b.digest)
    }
}

// LockfileMetadata

/// Informational metadata written to `bv.lock` by `bv lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileMetadata {
    pub bv_version: String,
    pub generated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_summary: Option<String>,
}

impl Default for LockfileMetadata {
    fn default() -> Self {
        Self {
            bv_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: Utc::now(),
            hardware_summary: None,
        }
    }
}

// Lockfile

/// The full `bv.lock` file.
///
/// Format is stable: `bv lock --check` fails if the generated lockfile
/// would differ from the on-disk one on any stability field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default)]
    pub metadata: LockfileMetadata,
    #[serde(default)]
    pub tools: BTreeMap<String, LockfileEntry>,
    /// Derived routing table: binary name -> tool id.
    /// Rebuilt by `rebuild_binary_index` whenever tools change.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary_index: BinaryIndex,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            version: 1,
            metadata: LockfileMetadata::default(),
            tools: BTreeMap::new(),
            binary_index: BTreeMap::new(),
        }
    }

    pub fn from_toml_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| BvError::LockfileParse(e.to_string()))
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| BvError::LockfileParse(e.to_string()))
    }

    /// Rebuild `binary_index` from each tool's `binaries` list.
    ///
    /// `overrides` maps binary name to the tool id that wins when two tools
    /// expose the same name. Without an override, a collision returns `Err`.
    pub fn rebuild_binary_index(
        &mut self,
        overrides: &BTreeMap<String, String>,
    ) -> std::result::Result<(), String> {
        let mut index: BinaryIndex = BTreeMap::new();
        let mut collisions: Vec<String> = Vec::new();

        let mut sorted: Vec<_> = self.tools.iter().collect();
        sorted.sort_by_key(|(id, _)| id.as_str());

        for (tool_id, entry) in &sorted {
            for binary in &entry.binaries {
                if let Some(winner) = overrides.get(binary) {
                    index.insert(binary.clone(), winner.clone());
                } else if let Some(existing) = index.insert(binary.clone(), tool_id.to_string())
                    && existing != tool_id.as_str()
                {
                    collisions.push(format!(
                        "'{binary}' exposed by both '{existing}' and '{tool_id}'"
                    ));
                    index.insert(binary.clone(), existing);
                }
            }
        }

        if !collisions.is_empty() {
            return Err(collisions.join(", "));
        }
        self.binary_index = index;
        Ok(())
    }

    /// True when both lockfiles describe the same set of tools at the same
    /// resolved versions and digests.
    pub fn is_equivalent_to(&self, other: &Self) -> bool {
        if self.tools.len() != other.tools.len() {
            return false;
        }
        for (id, entry) in &self.tools {
            match other.tools.get(id) {
                Some(other_entry) => {
                    if !entry.is_equivalent(other_entry) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str, digest: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: id.to_string(),
            declared_version_req: String::new(),
            version: version.to_string(),
            spec_kind: SpecKind::LegacyImage,
            image_reference: format!("registry/{id}:{version}"),
            image_digest: digest.to_string(),
            manifest_sha256: format!("sha256:m-{id}"),
            image_size_bytes: None,
            layers: vec![],
            resolved_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1700000000, 0).unwrap(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![format!("{id}-bin")],
        }
    }

    fn factored_entry(id: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: id.to_string(),
            declared_version_req: "=1.0.0".into(),
            version: "1.0.0".into(),
            spec_kind: SpecKind::FactoredOci,
            image_reference: format!("registry/{id}:1.0.0"),
            image_digest: format!("sha256:img-{id}"),
            manifest_sha256: format!("sha256:man-{id}"),
            image_size_bytes: None,
            layers: vec![
                LayerDescriptor {
                    digest: "sha256:shared-openssl".into(),
                    size: 10_000_000,
                    media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
                    conda_package: Some(CondaPackagePin {
                        name: "openssl".into(),
                        version: "3.2.1".into(),
                        build: "h0_0".into(),
                        channel: "conda-forge".into(),
                        sha256: "abcd".into(),
                    }),
                },
                LayerDescriptor {
                    digest: format!("sha256:pkg-{id}"),
                    size: 20_000_000,
                    media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
                    conda_package: None,
                },
            ],
            resolved_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1700000000, 0).unwrap(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![id.to_string()],
        }
    }

    /// Regression: lockfile serialization must be byte-deterministic so
    /// `bv lock --check` can compare against the on-disk file.
    #[test]
    fn to_toml_string_is_deterministic() {
        let mut lock = Lockfile::new();
        for id in ["zebra", "alpha", "mango", "beta", "tango"] {
            lock.tools.insert(
                id.to_string(),
                entry(id, "1.0.0", &format!("sha256:d-{id}")),
            );
            lock.binary_index
                .insert(format!("{id}-bin"), id.to_string());
        }

        let s1 = lock.to_toml_string().unwrap();
        for _ in 0..32 {
            assert_eq!(s1, lock.to_toml_string().unwrap(), "non-deterministic output");
        }
        // Tools must appear in lexicographic order.
        let alpha = s1.find("\"alpha\"").unwrap();
        let beta = s1.find("\"beta\"").unwrap();
        let mango = s1.find("\"mango\"").unwrap();
        let tango = s1.find("\"tango\"").unwrap();
        let zebra = s1.find("\"zebra\"").unwrap();
        assert!(alpha < beta && beta < mango && mango < tango && tango < zebra);
    }

    #[test]
    fn spec_kind_legacy_is_skipped_in_serialization() {
        let mut lock = Lockfile::new();
        lock.tools.insert("tool".into(), entry("tool", "1.0.0", "sha256:abc"));
        let s = lock.to_toml_string().unwrap();
        // Legacy entries must not emit spec_kind to keep backward compat.
        assert!(!s.contains("spec_kind"), "legacy entries must not emit spec_kind: {s}");
    }

    #[test]
    fn factored_entry_round_trips() {
        let mut lock = Lockfile::new();
        lock.tools.insert("samtools".into(), factored_entry("samtools"));
        let s = lock.to_toml_string().unwrap();
        let back = Lockfile::from_toml_str(&s).unwrap();
        let e = &back.tools["samtools"];
        assert_eq!(e.spec_kind, SpecKind::FactoredOci);
        assert_eq!(e.layers.len(), 2);
        assert_eq!(e.layers[0].conda_package.as_ref().unwrap().name, "openssl");
    }

    #[test]
    fn is_equivalent_checks_layer_digests() {
        let a = factored_entry("samtools");
        let mut b = a.clone();
        b.layers[0].digest = "sha256:different".into();
        assert!(!a.is_equivalent(&b));
    }

    #[test]
    fn is_equivalent_ignores_timestamps() {
        let a = factored_entry("samtools");
        let mut b = a.clone();
        b.resolved_at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        assert!(a.is_equivalent(&b));
    }
}

#[cfg(test)]
mod prop_tests {
    use proptest::prelude::*;

    use super::*;

    fn arb_tool_id() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{1,15}".prop_map(|s| s)
    }

    fn arb_digest() -> impl Strategy<Value = String> {
        "[0-9a-f]{64}".prop_map(|hex| format!("sha256:{hex}"))
    }

    fn arb_version() -> impl Strategy<Value = String> {
        (0u32..20, 0u32..20, 0u32..20).prop_map(|(a, b, c)| format!("{a}.{b}.{c}"))
    }

    fn arb_layer() -> impl Strategy<Value = LayerDescriptor> {
        (arb_digest(), 0u64..10_000_000u64).prop_map(|(digest, size)| LayerDescriptor {
            digest,
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
            conda_package: None,
        })
    }

    prop_compose! {
        fn arb_entry()(
            id in arb_tool_id(),
            version in arb_version(),
            digest in arb_digest(),
            manifest_sha256 in arb_digest(),
            size in proptest::option::of(0u64..10_000_000_000u64),
            layers in proptest::collection::vec(arb_layer(), 0..6),
        ) -> (String, LockfileEntry) {
            let spec_kind = if layers.is_empty() { SpecKind::LegacyImage } else { SpecKind::FactoredOci };
            let entry = LockfileEntry {
                tool_id: id.clone(),
                declared_version_req: format!("={version}"),
                version: version.clone(),
                spec_kind,
                image_reference: format!("registry/{id}:{version}"),
                image_digest: digest,
                manifest_sha256,
                image_size_bytes: size,
                layers,
                resolved_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1700000000, 0).unwrap(),
                reference_data_pins: BTreeMap::new(),
                binaries: vec![id.clone()],
            };
            (id, entry)
        }
    }

    prop_compose! {
        fn arb_lockfile()(
            entries in proptest::collection::vec(arb_entry(), 0..10),
        ) -> Lockfile {
            let mut lock = Lockfile::new();
            for (id, entry) in entries {
                lock.tools.insert(id, entry);
            }
            lock
        }
    }

    proptest! {
        /// Round-trip through TOML must be lossless on all stability fields.
        #[test]
        fn round_trip_preserves_all_fields(lock in arb_lockfile()) {
            let serialized = lock.to_toml_string().expect("serialize");
            let deserialized = Lockfile::from_toml_str(&serialized).expect("deserialize");

            prop_assert_eq!(lock.version, deserialized.version);
            prop_assert_eq!(lock.tools.len(), deserialized.tools.len());

            for (id, orig) in &lock.tools {
                let restored = deserialized.tools.get(id).expect("tool present after round-trip");
                prop_assert_eq!(&orig.tool_id, &restored.tool_id);
                prop_assert_eq!(&orig.version, &restored.version);
                prop_assert_eq!(&orig.image_reference, &restored.image_reference);
                prop_assert_eq!(&orig.image_digest, &restored.image_digest);
                prop_assert_eq!(&orig.manifest_sha256, &restored.manifest_sha256);
                prop_assert_eq!(orig.image_size_bytes, restored.image_size_bytes);
                prop_assert_eq!(orig.layers.len(), restored.layers.len());
                for (la, lb) in orig.layers.iter().zip(restored.layers.iter()) {
                    prop_assert_eq!(&la.digest, &lb.digest);
                    prop_assert_eq!(la.size, lb.size);
                }
            }
        }

        /// Serialization is deterministic: calling to_toml_string twice gives identical bytes.
        #[test]
        fn serialization_is_deterministic(lock in arb_lockfile()) {
            let s1 = lock.to_toml_string().expect("first serialize");
            let s2 = lock.to_toml_string().expect("second serialize");
            prop_assert_eq!(s1, s2);
        }

        /// Tool map keys appear in sorted (BTreeMap) order in the output.
        #[test]
        fn tool_keys_are_sorted(lock in arb_lockfile()) {
            if lock.tools.len() < 2 { return Ok(()); }
            let s = lock.to_toml_string().expect("serialize");
            let keys: Vec<&str> = lock.tools.keys().map(|k| k.as_str()).collect();
            let positions: Vec<usize> = keys
                .iter()
                .filter_map(|k| s.find(&format!("\"{k}\"")))
                .collect();
            prop_assert_eq!(positions.len(), lock.tools.len(), "all keys present");
            let mut sorted = positions.clone();
            sorted.sort_unstable();
            prop_assert_eq!(positions, sorted, "keys appear in sorted order");
        }

        /// No floating-point values appear in the serialized output.
        #[test]
        fn no_floats_in_output(lock in arb_lockfile()) {
            let s = lock.to_toml_string().expect("serialize");
            let has_float = s.lines().any(|line| {
                let line = line.trim();
                if line.contains('"') || line.contains('T') { return false; }
                if let Some(rhs) = line.split_once('=').map(|(_, v)| v.trim()) {
                    return rhs.starts_with(|c: char| c.is_ascii_digit()) && rhs.contains('.');
                }
                false
            });
            prop_assert!(!has_float, "float found in lockfile output:\n{s}");
        }

        /// Timestamps must be UTC ISO-8601 strings, not bare integers.
        #[test]
        fn timestamps_are_iso8601_utc(lock in arb_lockfile()) {
            let s = lock.to_toml_string().expect("serialize");
            for key in ["resolved_at", "generated_at"] {
                if let Some(line) = s.lines().find(|l| l.contains(key)) {
                    prop_assert!(
                        line.contains('Z') || line.contains("+00:00"),
                        "timestamp not UTC: {line}"
                    );
                }
            }
        }
    }
}
