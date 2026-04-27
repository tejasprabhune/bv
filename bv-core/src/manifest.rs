use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use bv_types::{Cardinality, TypeRef};

use crate::error::{BvError, Result};

/// Quality and governance tier for a tool in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Typed I/O complete, conformance tests pass, from a recognized publisher, actively maintained.
    Core,
    /// Typed I/O present (may be partial), basic checks pass.
    #[default]
    Community,
    /// Basic checks pass; may lack typed I/O. Hidden from default search results.
    Experimental,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Core => "core",
            Tier::Community => "community",
            Tier::Experimental => "experimental",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured CUDA version with ordering (`12.1 < 12.4 < 13.0`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CudaVersion {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for CudaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for CudaVersion {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (maj, min) = s
            .split_once('.')
            .ok_or_else(|| format!("expected 'major.minor', got '{s}'"))?;
        Ok(CudaVersion {
            major: maj
                .parse()
                .map_err(|_| format!("invalid major version '{maj}'"))?,
            minor: min
                .parse()
                .map_err(|_| format!("invalid minor version '{min}'"))?,
        })
    }
}

impl TryFrom<String> for CudaVersion {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<CudaVersion> for String {
    fn from(v: CudaVersion) -> String {
        v.to_string()
    }
}

impl Serialize for CudaVersion {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CudaVersion {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSpec {
    pub required: bool,
    pub min_vram_gb: Option<u32>,
    pub cuda_version: Option<CudaVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareSpec {
    pub gpu: Option<GpuSpec>,
    pub cpu_cores: Option<u32>,
    pub ram_gb: Option<f64>,
    pub disk_gb: Option<f64>,
}

impl HardwareSpec {
    /// Check this manifest's requirements against the host's detected hardware.
    /// Returns every requirement that is not satisfied.
    pub fn check_against(
        &self,
        detected: &crate::hardware::DetectedHardware,
    ) -> Vec<crate::hardware::HardwareMismatch> {
        use crate::hardware::HardwareMismatch;
        let mut out = Vec::new();

        if let Some(gpu_req) = &self.gpu
            && gpu_req.required
        {
            if detected.gpus.is_empty() {
                out.push(HardwareMismatch::NoGpu);
            } else {
                if let Some(min_vram) = gpu_req.min_vram_gb {
                    let best_vram_mb = detected.gpus.iter().map(|g| g.vram_mb).max().unwrap_or(0);
                    let best_vram_gb = (best_vram_mb as f64 / 1024.0).floor() as u32;
                    if best_vram_gb < min_vram {
                        out.push(HardwareMismatch::InsufficientVram {
                            required_gb: min_vram,
                            available_gb: best_vram_gb,
                        });
                    }
                }
                if let Some(min_cuda) = &gpu_req.cuda_version {
                    let best_cuda = detected
                        .gpus
                        .iter()
                        .filter_map(|g| g.cuda_version.as_ref())
                        .max();
                    match best_cuda {
                        None => out.push(HardwareMismatch::NoCuda {
                            required: min_cuda.clone(),
                        }),
                        Some(avail) if avail < min_cuda => {
                            out.push(HardwareMismatch::CudaTooOld {
                                required: min_cuda.clone(),
                                available: avail.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(min_ram) = self.ram_gb {
            let avail = detected.ram_gb();
            if avail < min_ram {
                out.push(HardwareMismatch::InsufficientRam {
                    required_gb: min_ram,
                    available_gb: avail,
                });
            }
        }

        if let Some(min_disk) = self.disk_gb {
            let avail = detected.disk_free_gb();
            if avail < min_disk {
                out.push(HardwareMismatch::InsufficientDisk {
                    required_gb: min_disk,
                    available_gb: avail,
                });
            }
        }

        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSpec {
    /// Runtime backend, e.g. `"docker"` or `"apptainer"`.
    pub backend: String,
    /// Canonical OCI reference, e.g. `"biocontainers/bwa:0.7.17"`.
    pub reference: String,
    /// Optional pinned digest for reproducibility.
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceDataSpec {
    pub id: String,
    pub version: String,
    pub required: bool,
    /// Container path where the dataset directory is mounted read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,
    /// Approximate compressed size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Typed I/O port declaration for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoSpec {
    pub name: String,
    /// Type reference, e.g. `"fasta"` or `"fasta[protein]"`.
    #[serde(rename = "type")]
    pub r#type: TypeRef,
    /// How many values this port accepts.
    #[serde(default)]
    pub cardinality: Cardinality,
    /// Absolute path inside the container where this value is mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrypointSpec {
    pub command: String,
    pub args_template: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Canonical inputs and expected outputs used by the conformance test runner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSpec {
    /// Map of port name to a `test://` URI identifying the canonical input.
    #[serde(default)]
    pub inputs: std::collections::HashMap<String, String>,
    /// Port names whose output files must exist and pass type-level checks.
    #[serde(default)]
    pub expected_outputs: Vec<String>,
    /// Additional CLI args appended to the entrypoint during test runs.
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Seconds before the conformance run is killed.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// When true, skipped in fast CI and run only on a separate slow schedule.
    #[serde(default)]
    pub slow: bool,
}

fn default_timeout() -> u64 {
    60
}

/// Optional Sigstore/cosign signature metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureSpec {
    /// `"sigstore"` to verify the OCI image signature with cosign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// `"sigstore"` to verify the manifest's commit signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: String,
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// Governance tier. Defaults to `community` for new submissions.
    #[serde(default)]
    pub tier: Tier,
    /// GitHub handles of maintainers, e.g. `"github:alice"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maintainers: Vec<String>,
    /// Set to `true` when a tool is superseded or no longer maintained.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
    pub image: ImageSpec,
    pub hardware: HardwareSpec,
    #[serde(default)]
    pub reference_data: HashMap<String, ReferenceDataSpec>,
    /// Typed inputs. Optional; manifests without this section parse unchanged.
    #[serde(default)]
    pub inputs: Vec<IoSpec>,
    /// Typed outputs. Optional; manifests without this section parse unchanged.
    #[serde(default)]
    pub outputs: Vec<IoSpec>,
    pub entrypoint: EntrypointSpec,
    /// Conformance test block; used by `bv conformance check`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<TestSpec>,
    /// Sigstore / cosign signature declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signatures: Option<SignatureSpec>,
}

impl ToolManifest {
    pub fn has_typed_io(&self) -> bool {
        !self.inputs.is_empty() || !self.outputs.is_empty()
    }
}

/// Top-level manifest, corresponding to a single `.toml` file in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub tool: ToolManifest,
}

#[derive(Debug)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl Manifest {
    pub fn from_toml_str(s: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(s).map_err(|e| BvError::ManifestParse(e.to_string()))?;
        m.validate_types()?;
        Ok(m)
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| BvError::ManifestParse(e.to_string()))
    }

    /// Validates that all TypeRefs in inputs/outputs exist in the bv-types vocabulary.
    fn validate_types(&self) -> Result<()> {
        let t = &self.tool;
        for (side, specs) in [("inputs", &t.inputs), ("outputs", &t.outputs)] {
            for spec in specs {
                let id = spec.r#type.base_id();
                if bv_types::lookup(id).is_none() {
                    let suggestion = bv_types::suggest(id)
                        .map(|s| format!(", did you mean `{s}`?"))
                        .unwrap_or_default();
                    return Err(BvError::ManifestParse(format!(
                        "tool.{side}[{}]: unknown type `{id}`{suggestion}",
                        spec.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Returns a list of validation errors, or `Ok(())` if the manifest is valid.
    pub fn validate(&self) -> std::result::Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        let t = &self.tool;

        if t.id.is_empty() {
            errors.push(ValidationError {
                field: "tool.id".into(),
                message: "must not be empty".into(),
            });
        }
        if t.version.is_empty() {
            errors.push(ValidationError {
                field: "tool.version".into(),
                message: "must not be empty".into(),
            });
        }
        if t.image.backend.is_empty() {
            errors.push(ValidationError {
                field: "tool.image.backend".into(),
                message: "must not be empty".into(),
            });
        }
        if t.image.reference.is_empty() {
            errors.push(ValidationError {
                field: "tool.image.reference".into(),
                message: "must not be empty".into(),
            });
        }
        if t.entrypoint.command.is_empty() {
            errors.push(ValidationError {
                field: "tool.entrypoint.command".into(),
                message: "must not be empty".into(),
            });
        }

        for spec in &t.inputs {
            if let Some(mount) = &spec.mount {
                if !mount.is_absolute() {
                    errors.push(ValidationError {
                        field: format!("tool.inputs[{}].mount", spec.name),
                        message: "must be an absolute path".into(),
                    });
                }
            }
        }
        for spec in &t.outputs {
            if let Some(mount) = &spec.mount {
                if !mount.is_absolute() {
                    errors.push(ValidationError {
                        field: format!("tool.outputs[{}].mount", spec.name),
                        message: "must be an absolute path".into(),
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[tool]
id = "bwa"
version = "0.7.17"
description = "BWA short-read aligner"
homepage = "http://bio-bwa.sourceforge.net/"
license = "GPL-3.0"

[tool.image]
backend = "docker"
reference = "biocontainers/bwa:0.7.17--h5bf99c6_8"
digest = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab"

[tool.hardware]
cpu_cores = 8
ram_gb = 32.0
disk_gb = 50.0

[tool.hardware.gpu]
required = false

[[tool.inputs]]
name = "reads_r1"
type = "fastq"
cardinality = "one"
description = "Forward reads"

[[tool.inputs]]
name = "reads_r2"
type = "fastq"
cardinality = "optional"
description = "Reverse reads (paired-end)"

[[tool.outputs]]
name = "alignment"
type = "bam"
description = "Aligned reads"

[tool.entrypoint]
command = "bwa"
args_template = "mem -t {cpu_cores} {reference} {reads_r1} {reads_r2}"

[tool.entrypoint.env]
MALLOC_ARENA_MAX = "4"
"#;

    const SAMPLE_NO_IO: &str = r#"
[tool]
id = "mytool"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/mytool:1.0.0"

[tool.hardware]

[tool.entrypoint]
command = "mytool"
"#;

    #[test]
    fn round_trip() {
        let manifest = Manifest::from_toml_str(SAMPLE).expect("parse failed");
        assert_eq!(manifest.tool.id, "bwa");
        assert_eq!(manifest.tool.version, "0.7.17");
        assert_eq!(manifest.tool.image.backend, "docker");
        assert_eq!(manifest.tool.inputs.len(), 2);
        assert_eq!(manifest.tool.outputs.len(), 1);
        assert_eq!(manifest.tool.inputs[0].cardinality, Cardinality::One);
        assert_eq!(manifest.tool.inputs[1].cardinality, Cardinality::Optional);

        let serialised = manifest.to_toml_string().expect("serialise failed");
        let reparsed = Manifest::from_toml_str(&serialised).expect("reparse failed");
        assert_eq!(reparsed.tool.id, manifest.tool.id);
        assert_eq!(reparsed.tool.version, manifest.tool.version);
    }

    #[test]
    fn no_io_parses_unchanged() {
        let m = Manifest::from_toml_str(SAMPLE_NO_IO).expect("parse failed");
        assert!(m.tool.inputs.is_empty());
        assert!(m.tool.outputs.is_empty());
        assert!(!m.tool.has_typed_io());
    }

    #[test]
    fn typeref_params_parsed() {
        let s = r#"
[tool]
id = "t"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/t:1.0.0"

[tool.hardware]

[[tool.inputs]]
name = "seqs"
type = "fasta[protein]"
cardinality = "one"

[tool.entrypoint]
command = "t"
"#;
        let m = Manifest::from_toml_str(s).unwrap();
        assert_eq!(m.tool.inputs[0].r#type.params, vec!["protein"]);
    }

    #[test]
    fn unknown_type_error() {
        let s = r#"
[tool]
id = "t"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/t:1.0.0"

[tool.hardware]

[[tool.inputs]]
name = "seqs"
type = "protien_fasta"
cardinality = "one"

[tool.entrypoint]
command = "t"
"#;
        let err = Manifest::from_toml_str(s).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown type"), "got: {msg}");
    }

    #[test]
    fn cuda_version_ordering() {
        let v12_1: CudaVersion = "12.1".parse().unwrap();
        let v12_4: CudaVersion = "12.4".parse().unwrap();
        let v13_0: CudaVersion = "13.0".parse().unwrap();
        assert!(v12_1 < v12_4);
        assert!(v12_4 < v13_0);
        assert_eq!(v12_1, "12.1".parse::<CudaVersion>().unwrap());
    }

    #[test]
    fn validate_catches_empty_id() {
        let mut manifest = Manifest::from_toml_str(SAMPLE).unwrap();
        manifest.tool.id = String::new();
        let errs = manifest.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.field == "tool.id"));
    }

    #[test]
    fn registry_manifests_parse() {
        let registry = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bv-registry/tools");
        for entry in std::fs::read_dir(registry).unwrap() {
            let tool_dir = entry.unwrap().path();
            if !tool_dir.is_dir() {
                continue;
            }
            for version_entry in std::fs::read_dir(&tool_dir).unwrap() {
                let path = version_entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "toml") {
                    let s = std::fs::read_to_string(&path)
                        .unwrap_or_else(|_| panic!("failed to read {}", path.display()));
                    Manifest::from_toml_str(&s)
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                }
            }
        }
    }
}
