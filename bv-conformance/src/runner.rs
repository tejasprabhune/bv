use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use bv_core::manifest::Manifest;
use bv_runtime::{ContainerRuntime, GpuProfile, ImageDigest, Mount, OciRef, RunSpec};

pub struct ConformanceResult {
    pub tool_id: String,
    pub passed: bool,
    pub messages: Vec<String>,
    pub duration: Duration,
}

impl ConformanceResult {
    fn pass(tool_id: impl Into<String>, messages: Vec<String>, duration: Duration) -> Self {
        Self {
            tool_id: tool_id.into(),
            passed: true,
            messages,
            duration,
        }
    }
    fn fail(tool_id: impl Into<String>, messages: Vec<String>, duration: Duration) -> Self {
        Self {
            tool_id: tool_id.into(),
            passed: false,
            messages,
            duration,
        }
    }
}

/// Probe args we try, in order, when smoke-checking a binary. We accept two
/// signals as "alive": exit code 0, OR substantial output to stdout/stderr.
/// The latter catches tools that follow the Unix convention of "print help,
/// exit non-zero" for unknown args (bwa, seqtk, fasttree). A binary that
/// segfaulted on load would produce neither, so this is a safe relaxation.
const DEFAULT_PROBES: &[&str] = &["--version", "-version", "--help", "-h", "-v", "version", ""];

/// Minimum bytes of stdout+stderr to count a probe as "produced output".
/// Tuned to filter out noise like a single newline or a one-line "command not
/// found" while accepting any real help/version blurb (typically >100 bytes).
const ALIVE_OUTPUT_THRESHOLD: usize = 30;

/// Run the smoke check for a manifest using the given runtime.
///
/// For every binary in `[tool.binaries]` (or the entrypoint command for
/// single-binary tools), try a small set of probe args. A binary passes
/// if any probe exits 0. Tool authors can override the probe list per
/// binary, or skip individual binaries, via `[tool.smoke]`.
///
/// Returns `Ok(result)` even on conformance failures; `Err` only on setup
/// errors (e.g. tempdir creation).
pub fn run(
    manifest: &Manifest,
    image_digest: &str,
    runtime: &dyn ContainerRuntime,
) -> anyhow::Result<ConformanceResult> {
    let tool = &manifest.tool;
    let start = std::time::Instant::now();

    let failures = check_binaries(manifest, image_digest, runtime);

    let duration = start.elapsed();
    if failures.is_empty() {
        Ok(ConformanceResult::pass(
            &tool.id,
            vec!["all binaries responded to smoke probes".into()],
            duration,
        ))
    } else {
        Ok(ConformanceResult::fail(&tool.id, failures, duration))
    }
}

/// For each binary, try probes until one is accepted. A probe is accepted
/// if the binary exits 0 OR produces ≥ALIVE_OUTPUT_THRESHOLD bytes on
/// stdout/stderr. Manifest-declared `[tool.smoke]` overrides take precedence:
/// a `probes` entry pins the probe to one specific arg, and a `skip` entry
/// omits the binary entirely.
fn check_binaries(
    manifest: &Manifest,
    image_digest: &str,
    runtime: &dyn ContainerRuntime,
) -> Vec<String> {
    let tool = &manifest.tool;
    let binaries = tool.effective_binaries();

    let mut image: OciRef = match tool.image.reference.parse() {
        Ok(r) => r,
        Err(_) => return vec!["invalid image reference; cannot run smoke check".into()],
    };
    image.tag = None;
    image.digest = Some(image_digest.to_string());

    let tmp = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(e) => return vec![format!("failed to create temp workspace: {e}")],
    };

    let smoke = tool.smoke.clone().unwrap_or_default();
    let mut failures = Vec::new();

    for binary in binaries {
        if smoke.skip.iter().any(|s| s == binary) {
            continue;
        }

        // If the manifest pins a probe for this binary, only try that one.
        // Otherwise try the default list and accept the first exit 0.
        let probes: Vec<String> = match smoke.probes.get(binary) {
            Some(probe) => vec![probe.clone()],
            None => DEFAULT_PROBES.iter().map(|s| s.to_string()).collect(),
        };

        let mut passed = false;
        for probe in &probes {
            let mut command = vec![binary.to_string()];
            if !probe.is_empty() {
                command.push(probe.clone());
            }
            let spec = RunSpec {
                image: image.clone(),
                command,
                env: Default::default(),
                mounts: vec![Mount {
                    host_path: tmp.path().to_path_buf(),
                    container_path: PathBuf::from("/workspace"),
                    read_only: false,
                }],
                gpu: GpuProfile { spec: None },
                working_dir: Some(PathBuf::from("/workspace")),
                capture_output: true,
            };
            if let Ok(outcome) = runtime.run(&spec)
                && (outcome.exit_code == 0
                    || outcome.stdout.len() + outcome.stderr.len() >= ALIVE_OUTPUT_THRESHOLD)
            {
                passed = true;
                break;
            }
        }

        if !passed {
            let probe_list = probes
                .iter()
                .map(|p| {
                    if p.is_empty() {
                        "(no args)".into()
                    } else {
                        format!("'{p}'")
                    }
                })
                .collect::<Vec<_>>()
                .join(" / ");
            failures.push(format!(
                "binary '{binary}' did not respond to {probe_list}.\n  \
                 If this is expected (no version/help arg), add it to [tool.smoke].skip\n  \
                 in the manifest. Or pin the right probe via [tool.smoke].probes."
            ));
        }
    }
    failures
}

/// Pull the image and verify it is reachable. Returns the digest.
pub fn verify_image_reachable(
    manifest: &Manifest,
    runtime: &dyn ContainerRuntime,
) -> anyhow::Result<ImageDigest> {
    let oci_ref: OciRef = manifest
        .tool
        .image
        .reference
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid image ref: {e}"))?;

    let reporter = bv_runtime::NoopProgress;
    runtime
        .pull(&oci_ref, &reporter)
        .with_context(|| format!("failed to pull image for '{}'", manifest.tool.id))
}
