use anyhow::Context;
use owo_colors::{OwoColorize, Stream};
use semver::VersionReq;

use bv_core::cache::CacheLayout;
use bv_index::IndexBackend as _;
use bv_runtime::ContainerRuntime as _;

use crate::registry::{open_index, resolve_registry_url};

pub async fn run(
    tool: &str,
    registry_flag: Option<&str>,
    backend_flag: Option<&str>,
) -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let registry_url = resolve_registry_url(registry_flag, None);
    let index = open_index(&registry_url, &cache);

    index.refresh().context("registry refresh failed")?;

    let manifest = index
        .get_manifest(tool, &VersionReq::STAR)
        .with_context(|| format!("tool '{}' not found in registry", tool))?;

    let runtime = crate::runtime_select::resolve_runtime(backend_flag, None)?;

    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    eprintln!(
        "  {} {}@{}",
        "Running conformance for"
            .if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        tool,
        manifest.tool.version
    );

    // Pull the image first.
    let image_digest = bv_conformance::verify_image_reachable(
        &manifest,
        &runtime as &dyn bv_runtime::ContainerRuntime,
    )
    .with_context(|| format!("failed to pull image for '{}'", tool))?;

    eprintln!(
        "  {} image pulled ({})",
        "ok".if_supports_color(Stream::Stderr, |t| t.green().to_string()),
        &image_digest.0[..image_digest.0.len().min(20)]
    );

    // Run the conformance tests.
    let result = bv_conformance::run(
        &manifest,
        &image_digest.0,
        &runtime as &dyn bv_runtime::ContainerRuntime,
    )?;

    let status_label = if result.passed {
        "ok".if_supports_color(Stream::Stderr, |t| t.green().to_string())
            .to_string()
    } else {
        "fail"
            .if_supports_color(Stream::Stderr, |t| t.red().to_string())
            .to_string()
    };
    for msg in &result.messages {
        eprintln!("    {} {}", status_label, msg);
    }

    let outcome = if result.passed {
        "PASSED"
            .if_supports_color(Stream::Stderr, |t| t.green().bold().to_string())
            .to_string()
    } else {
        "FAILED"
            .if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
            .to_string()
    };
    eprintln!(
        "\n  {} {} in {:.1}s",
        outcome,
        tool,
        result.duration.as_secs_f32()
    );

    if !result.passed {
        anyhow::bail!("conformance failed for '{}'", tool);
    }
    Ok(())
}
