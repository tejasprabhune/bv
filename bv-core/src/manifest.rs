use std::collections::BTreeMap;
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
                    // Round to nearest GiB instead of floor: nvidia-smi
                    // typically reports just-under marketing capacity (e.g.
                    // 24268 MiB on a "24 GB" RTX 3090). flooring made
                    // min_vram_gb=24 spuriously fail on real hardware.
                    let best_vram_gb = ((best_vram_mb as f64) / 1024.0).round() as u32;
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
    pub env: BTreeMap<String, String>,
}

/// Binary names that the tool's container exposes on PATH.
///
/// Omitting this block defaults to `exposed = [entrypoint.command]` for
/// single-binary tools that do not need to declare anything extra.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinariesSpec {
    pub exposed: Vec<String>,
}

/// Per-tool overrides for `bv conformance`'s smoke check.
///
/// The smoke check tries a small set of probe args (`--version`, `-version`,
/// `--help`, `-h`, `-v`, `version`) against every binary the tool exposes,
/// and counts a binary as alive if any probe produces output or exits 0.
/// Most tools don't need a `[tool.smoke]` block at all; this is the escape
/// hatch for the unusual cases.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmokeSpec {
    /// Override probe args for specific binaries, e.g. `{ "blastn" = "-version" }`.
    /// Each value is a single command-line argument (or empty string for "run
    /// the binary with no args"). When set, only this probe is tried for that
    /// binary; the default list is bypassed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub probes: std::collections::BTreeMap<String, String>,
    /// Binaries to skip entirely (daemons, "no non-destructive invocation"
    /// tools, etc.). Listed binaries still appear in `[tool.binaries]` and
    /// get shims; conformance just doesn't probe them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<String>,
}

#[allow(dead_code)]
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
    pub reference_data: BTreeMap<String, ReferenceDataSpec>,
    /// Typed inputs. Optional; manifests without this section parse unchanged.
    #[serde(default)]
    pub inputs: Vec<IoSpec>,
    /// Typed outputs. Optional; manifests without this section parse unchanged.
    #[serde(default)]
    pub outputs: Vec<IoSpec>,
    /// Default invocation. Required unless `[tool.subcommands]` is non-empty;
    /// see `validate()`. Multi-script tools may omit this entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<EntrypointSpec>,
    /// Tool-namespaced launchers. Reachable as `bv run <toolid> <name> ...args`.
    /// Each value is the literal argv prefix; user args are appended verbatim.
    /// Unlike `[tool.binaries]`, names are not exposed on PATH or in the global
    /// binary index, so generic names (`train`, `eval`) are safe.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subcommands: BTreeMap<String, Vec<String>>,
    /// Container paths the tool writes to during normal execution and that
    /// should therefore be bound to writable host directories. Critical on
    /// apptainer (read-only SIF root), nice-to-have on docker (lets caches
    /// outlive `docker rm`). Tool authors declare these; users can override
    /// the host side via `[[cache]]` in `bv.toml`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_paths: Vec<String>,
    /// Binary names this tool exposes on PATH inside its container.
    /// Omit for single-binary tools; defaults to `[entrypoint.command]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binaries: Option<BinariesSpec>,
    /// Smoke-check overrides; consulted by `bv conformance` for unusual binaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smoke: Option<SmokeSpec>,
    /// Sigstore / cosign signature declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signatures: Option<SignatureSpec>,
}

impl ToolManifest {
    pub fn has_typed_io(&self) -> bool {
        !self.inputs.is_empty() || !self.outputs.is_empty()
    }

    /// Returns the effective list of binary names this tool exposes.
    ///
    /// When `[tool.binaries]` is absent, defaults to the entrypoint command's
    /// basename. Multi-script tools without an entrypoint expose no binaries
    /// (their subcommands stay namespaced under the tool id).
    pub fn effective_binaries(&self) -> Vec<&str> {
        if let Some(b) = &self.binaries {
            return b.exposed.iter().map(|s| s.as_str()).collect();
        }
        let Some(ep) = &self.entrypoint else {
            return vec![];
        };
        let cmd = &ep.command;
        let name = cmd
            .rfind('/')
            .map(|i| &cmd[i + 1..])
            .unwrap_or(cmd.as_str());
        vec![name]
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
        if let Err(errs) = m.validate() {
            let combined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(BvError::ManifestParse(format!(
                "manifest validation failed: {combined}"
            )));
        }
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
        match (&t.entrypoint, t.subcommands.is_empty()) {
            (None, true) => errors.push(ValidationError {
                field: "tool.entrypoint".into(),
                message: "must declare either [tool.entrypoint] or [tool.subcommands]".into(),
            }),
            (Some(ep), _) if ep.command.is_empty() => errors.push(ValidationError {
                field: "tool.entrypoint.command".into(),
                message: "must not be empty".into(),
            }),
            _ => {}
        }

        for (name, cmd) in &t.subcommands {
            if name.is_empty() {
                errors.push(ValidationError {
                    field: "tool.subcommands".into(),
                    message: "subcommand name must not be empty".into(),
                });
                continue;
            }
            if name.starts_with('-') {
                errors.push(ValidationError {
                    field: format!("tool.subcommands.{name}"),
                    message: "subcommand name must not start with '-'".into(),
                });
            }
            if cmd.is_empty() {
                errors.push(ValidationError {
                    field: format!("tool.subcommands.{name}"),
                    message: "command vector must not be empty".into(),
                });
            }
        }

        for spec in &t.inputs {
            if let Some(mount) = &spec.mount
                && !mount.is_absolute()
            {
                errors.push(ValidationError {
                    field: format!("tool.inputs[{}].mount", spec.name),
                    message: "must be an absolute path".into(),
                });
            }
        }
        for spec in &t.outputs {
            if let Some(mount) = &spec.mount
                && !mount.is_absolute()
            {
                errors.push(ValidationError {
                    field: format!("tool.outputs[{}].mount", spec.name),
                    message: "must be an absolute path".into(),
                });
            }
        }

        if let Some(binaries) = &t.binaries {
            let mut seen = std::collections::HashSet::new();
            for name in &binaries.exposed {
                if !seen.insert(name.as_str()) {
                    errors.push(ValidationError {
                        field: "tool.binaries.exposed".into(),
                        message: format!("duplicate binary name '{name}'"),
                    });
                }
            }
            if !binaries.exposed.is_empty()
                && let Some(ep) = &t.entrypoint
            {
                let cmd = &ep.command;
                let basename = cmd.rfind('/').map(|i| &cmd[i + 1..]).unwrap_or(cmd);
                if !binaries.exposed.iter().any(|b| b == basename) {
                    errors.push(ValidationError {
                        field: "tool.binaries.exposed".into(),
                        message: format!(
                            "entrypoint command '{basename}' must be listed in exposed"
                        ),
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

    /// Regression: HashMap-backed fields produced non-deterministic TOML
    /// output, breaking lockfile drift detection. Re-serializing the same
    /// manifest must always yield identical bytes.
    #[test]
    fn to_toml_string_is_deterministic_with_subcommands() {
        let s = r#"
[tool]
id = "multi"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/multi:1.0.0"

[tool.hardware]

[tool.entrypoint]
command = "main"

[tool.subcommands]
zebra = ["script_z.py"]
alpha = ["script_a.py"]
mango = ["python", "-m", "scripts.mango"]
beta = ["script_b.py"]
"#;
        let m = Manifest::from_toml_str(s).expect("parse");
        let a = m.to_toml_string().unwrap();
        // Re-serialize many times to make iteration-order luck unlikely.
        for _ in 0..32 {
            assert_eq!(a, m.to_toml_string().unwrap(), "non-deterministic output");
        }
        // And the keys must appear in lexicographic order (BTreeMap).
        let alpha = a.find("alpha = ").unwrap();
        let beta = a.find("beta = ").unwrap();
        let mango = a.find("mango = ").unwrap();
        let zebra = a.find("zebra = ").unwrap();
        assert!(alpha < beta && beta < mango && mango < zebra);
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
    fn subcommands_only_parses() {
        let s = r#"
[tool]
id = "genie2"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "ghcr.io/example/genie2:1.0.0"

[tool.hardware]

[tool.subcommands]
train                = ["python", "genie/train.py"]
sample_unconditional = ["python", "genie/sample_unconditional.py"]
"#;
        let m = Manifest::from_toml_str(s).unwrap();
        assert!(m.tool.entrypoint.is_none());
        assert_eq!(m.tool.subcommands.len(), 2);
        assert_eq!(
            m.tool.subcommands.get("train").unwrap(),
            &vec!["python".to_string(), "genie/train.py".to_string()]
        );
        m.validate().expect("subcommand-only manifest is valid");
        // No entrypoint and no [tool.binaries] => no exposed binaries.
        assert!(m.tool.effective_binaries().is_empty());
    }

    #[test]
    fn validate_requires_entrypoint_or_subcommands() {
        let s = r#"
[tool]
id = "broken"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/broken:1.0.0"

[tool.hardware]
"#;
        // from_toml_str now runs validate(); manifest with neither entrypoint
        // nor subcommands must be rejected at parse time.
        let err = Manifest::from_toml_str(s).unwrap_err();
        assert!(
            err.to_string().contains("tool.entrypoint"),
            "expected entrypoint-or-subcommands error, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_dash_prefixed_subcommand() {
        let s = r#"
[tool]
id = "t"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/t:1.0.0"

[tool.hardware]

[tool.subcommands]
"-bad" = ["python", "x.py"]
"#;
        let err = Manifest::from_toml_str(s).unwrap_err();
        assert!(err.to_string().contains("-bad"), "got: {err}");
    }

    #[test]
    fn subcommands_round_trip() {
        let s = r#"
[tool]
id = "t"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "example/t:1.0.0"

[tool.hardware]

[tool.subcommands]
go = ["python", "main.py"]
"#;
        let m = Manifest::from_toml_str(s).unwrap();
        let serialised = m.to_toml_string().unwrap();
        let reparsed = Manifest::from_toml_str(&serialised).unwrap();
        assert_eq!(reparsed.tool.subcommands.len(), 1);
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
        let Ok(read) = std::fs::read_dir(registry) else {
            return;
        };
        for entry in read {
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
