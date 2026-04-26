use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{BvError, Result};

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
                    let best_vram_mb =
                        detected.gpus.iter().map(|g| g.vram_mb).max().unwrap_or(0);
                    let best_vram_gb = (best_vram_mb as f64 / 1024.0).floor() as u32;
                    if best_vram_gb < min_vram {
                        out.push(HardwareMismatch::InsufficientVram {
                            required_gb: min_vram,
                            available_gb: best_vram_gb,
                        });
                    }
                }
                if let Some(min_cuda) = &gpu_req.cuda_version {
                    let best_cuda =
                        detected.gpus.iter().filter_map(|g| g.cuda_version.as_ref()).max();
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoInput {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoOutput {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IoSpec {
    #[serde(default)]
    pub inputs: Vec<IoInput>,
    #[serde(default)]
    pub outputs: Vec<IoOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrypointSpec {
    pub command: String,
    pub args_template: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: String,
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub image: ImageSpec,
    pub hardware: HardwareSpec,
    #[serde(default)]
    pub reference_data: HashMap<String, ReferenceDataSpec>,
    #[serde(default)]
    pub io: IoSpec,
    pub entrypoint: EntrypointSpec,
}

/// Top-level manifest - corresponds to a single `manifest.toml` in the registry.
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
        toml::from_str(s).map_err(|e| BvError::ManifestParse(e.to_string()))
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| BvError::ManifestParse(e.to_string()))
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

[tool.reference_data.hg38]
id = "hg38"
version = "1.0"
required = true

[[tool.io.inputs]]
name = "reads_r1"
type = "fastq"
required = true
description = "Forward reads"

[[tool.io.inputs]]
name = "reads_r2"
type = "fastq"
required = false
description = "Reverse reads (paired-end)"

[[tool.io.outputs]]
name = "alignment"
type = "bam"
description = "Aligned reads"

[tool.entrypoint]
command = "bwa"
args_template = "mem -t {cpu_cores} {reference} {reads_r1} {reads_r2}"

[tool.entrypoint.env]
MALLOC_ARENA_MAX = "4"
"#;

    #[test]
    fn round_trip() {
        let manifest = Manifest::from_toml_str(SAMPLE).expect("parse failed");
        assert_eq!(manifest.tool.id, "bwa");
        assert_eq!(manifest.tool.version, "0.7.17");
        assert_eq!(manifest.tool.image.backend, "docker");
        assert_eq!(manifest.tool.io.inputs.len(), 2);
        assert_eq!(manifest.tool.io.outputs.len(), 1);

        let serialised = manifest.to_toml_string().expect("serialise failed");
        let reparsed = Manifest::from_toml_str(&serialised).expect("reparse failed");
        assert_eq!(reparsed.tool.id, manifest.tool.id);
        assert_eq!(reparsed.tool.version, manifest.tool.version);
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
}
