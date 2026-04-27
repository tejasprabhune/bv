use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use bv_core::manifest::{Manifest, TestSpec};
use bv_runtime::{ContainerRuntime, GpuProfile, ImageDigest, Mount, OciRef, RunSpec};
use tempfile::TempDir;

use crate::{assertions, inputs};

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

/// Run the conformance test for a manifest using the given runtime.
/// Returns `Ok(result)` even on conformance failures; `Err` means a setup
/// error (e.g. the image could not be pulled at all).
pub fn run(
    manifest: &Manifest,
    image_digest: &str,
    runtime: &dyn ContainerRuntime,
) -> anyhow::Result<ConformanceResult> {
    let tool = &manifest.tool;
    let start = std::time::Instant::now();

    let test_spec: &TestSpec = match &tool.test {
        Some(t) => t,
        None => {
            return Ok(ConformanceResult::pass(
                &tool.id,
                vec!["no [tool.test] block; skipping".into()],
                start.elapsed(),
            ));
        }
    };

    let tmp = TempDir::new().context("failed to create temp workspace")?;
    let workspace = tmp.path();

    // Materialize test inputs.
    let input_paths = inputs::materialize_all(&test_spec.inputs, workspace)
        .context("failed to materialize test inputs")?;

    // Build the run spec.
    let mut image: OciRef = tool
        .image
        .reference
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid image ref: {e}"))?;
    image.tag = None;
    image.digest = Some(image_digest.to_string());

    let mut command = vec![tool.entrypoint.command.clone()];
    command.extend(test_spec.extra_args.iter().cloned());

    // Substitute {port_name} placeholders with container paths.
    let command = substitute_placeholders(
        command,
        &tool.entrypoint.args_template,
        &input_paths,
        &tool.outputs,
        workspace,
    );

    let spec = RunSpec {
        image,
        command,
        env: tool.entrypoint.env.clone(),
        mounts: vec![Mount {
            host_path: workspace.to_path_buf(),
            container_path: PathBuf::from("/workspace"),
            read_only: false,
        }],
        gpu: GpuProfile {
            spec: tool.hardware.gpu.clone(),
        },
        working_dir: Some(PathBuf::from("/workspace")),
    };

    let outcome = runtime
        .run(&spec)
        .with_context(|| format!("failed to run '{}' during conformance test", tool.id))?;

    if outcome.exit_code != 0 {
        return Ok(ConformanceResult::fail(
            &tool.id,
            vec![format!("tool exited with code {}", outcome.exit_code)],
            start.elapsed(),
        ));
    }

    // Check declared outputs.
    let mut failures = Vec::new();
    for output_name in &test_spec.expected_outputs {
        let spec = tool.outputs.iter().find(|o| &o.name == output_name);
        let Some(output_spec) = spec else {
            failures.push(format!(
                "output '{}' declared in test block but not in [[tool.outputs]]",
                output_name
            ));
            continue;
        };

        let output_path = output_spec
            .mount
            .as_deref()
            .map(|m| workspace.join(m.strip_prefix("/workspace/").unwrap_or(m)))
            .unwrap_or_else(|| workspace.join(output_name));

        if let Err(e) = assertions::check_output(output_spec, &output_path) {
            failures.push(format!("{}: {e}", output_name));
        }
    }

    // Check that every declared binary responds to --help / --version / -h.
    let binary_failures = check_binaries(manifest, image_digest, runtime);
    failures.extend(binary_failures);

    let duration = start.elapsed();
    if failures.is_empty() {
        Ok(ConformanceResult::pass(
            &tool.id,
            vec!["all checks passed".into()],
            duration,
        ))
    } else {
        Ok(ConformanceResult::fail(&tool.id, failures, duration))
    }
}

/// For each binary in `tool.binaries.exposed`, verify it runs with
/// `--help`, `--version`, or `-h` and exits 0.
fn check_binaries(
    manifest: &Manifest,
    image_digest: &str,
    runtime: &dyn ContainerRuntime,
) -> Vec<String> {
    let tool = &manifest.tool;
    let binaries = tool.effective_binaries();

    let mut image: OciRef = match tool.image.reference.parse() {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    image.tag = None;
    image.digest = Some(image_digest.to_string());

    let tmp = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let mut failures = Vec::new();
    for binary in binaries {
        let mut passed = false;
        for probe in &["--help", "--version", "-h"] {
            let spec = RunSpec {
                image: image.clone(),
                command: vec![binary.to_string(), probe.to_string()],
                env: Default::default(),
                mounts: vec![Mount {
                    host_path: tmp.path().to_path_buf(),
                    container_path: PathBuf::from("/workspace"),
                    read_only: false,
                }],
                gpu: GpuProfile { spec: None },
                working_dir: Some(PathBuf::from("/workspace")),
            };
            if let Ok(outcome) = runtime.run(&spec)
                && outcome.exit_code == 0
            {
                passed = true;
                break;
            }
        }
        if !passed {
            failures.push(format!(
                "binary '{binary}' did not respond to --help / --version / -h with exit 0"
            ));
        }
    }
    failures
}

fn substitute_placeholders(
    mut command: Vec<String>,
    args_template: &Option<String>,
    input_paths: &std::collections::HashMap<String, PathBuf>,
    outputs: &[bv_core::manifest::IoSpec],
    workspace: &Path,
) -> Vec<String> {
    let Some(template) = args_template else {
        return command;
    };

    let mut expanded = template.clone();

    // Substitute input port placeholders: {port} → /workspace/<filename>
    for (port, path) in input_paths {
        let container_path = format!(
            "/workspace/{}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        expanded = expanded.replace(&format!("{{{port}}}"), &container_path);
    }

    // Substitute output port placeholders: {port} → /workspace/<port_name>
    // This must run before the generic {output} fallback below.
    for output_spec in outputs {
        let container_path = format!("/workspace/{}", output_spec.name);
        expanded = expanded.replace(&format!("{{{}}}", output_spec.name), &container_path);
    }

    expanded = expanded.replace("{cpu_cores}", "1");
    // Generic {output} fallback for manifests that don't name their output port.
    expanded = expanded.replace(
        "{output}",
        &format!(
            "/workspace/{}_output.txt",
            command.first().map(|s| s.as_str()).unwrap_or("out")
        ),
    );

    let _ = workspace;
    let args: Vec<String> = expanded.split_whitespace().map(str::to_string).collect();
    command.extend(args);
    command
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
