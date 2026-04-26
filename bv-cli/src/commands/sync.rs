use std::collections::HashSet;

use anyhow::Context;
use indicatif::MultiProgress;
use owo_colors::{OwoColorize, Stream};
use semver::VersionReq;

use bv_core::project::{BvLock, BvToml};
use bv_index::IndexBackend as _;
use bv_runtime::{ContainerRuntime, DockerRuntime, OciRef};

use crate::commands::add::short_digest;
use crate::ops;
use crate::progress::CliProgressReporter;

pub async fn run(frozen: bool, registry_flag: Option<&str>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!("no bv.lock found\n  run `bv add <tool>` or `bv lock` first");
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    if lockfile.tools.is_empty() {
        eprintln!("  bv.lock has no tools");
        return Ok(());
    }

    // --frozen: verify bv.toml and bv.lock are consistent before pulling.
    if frozen {
        let bv_toml_path = cwd.join("bv.toml");
        if !bv_toml_path.exists() {
            anyhow::bail!("--frozen requires bv.toml to be present");
        }
        let bv_toml = BvToml::from_path(&bv_toml_path)?;
        check_frozen(&bv_toml, &lockfile)?;
    }

    // Drift detection: if registry is available, compare manifest sha256.
    // This is best-effort; we proceed even if the registry is unreachable.
    let drift_warnings = check_drift(&lockfile, registry_flag).unwrap_or_default();

    let runtime = DockerRuntime;

    // Verify Docker is available.
    runtime
        .health_check()
        .context("Docker is not available. Is Docker Desktop running?")?;

    let mp = MultiProgress::new();
    let mut errors: Vec<String> = Vec::new();

    let mut sorted_tools: Vec<_> = lockfile.tools.values().collect();
    sorted_tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));

    for entry in sorted_tools {
        let base_ref = base_image_ref(&entry.image_reference);

        if runtime.is_locally_available(&base_ref, &entry.image_digest) {
            eprintln!(
                "  {} {} {}",
                "Present".if_supports_color(Stream::Stderr, |t| t.green().to_string()),
                entry
                    .tool_id
                    .if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
                entry
                    .version
                    .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
            );
            continue;
        }

        // Pull by pinned digest for reproducibility.
        let mut oci_ref: OciRef = entry
            .image_reference
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid image_reference in lockfile: {e}"))?;
        oci_ref.tag = None;
        oci_ref.digest = Some(entry.image_digest.clone());

        eprintln!(
            "  {} {} {}",
            "Pulling".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
            entry
                .tool_id
                .if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
            oci_ref
                .docker_arg()
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        );

        let reporter = CliProgressReporter::for_multi(&mp);
        match runtime.pull(&oci_ref, &reporter) {
            Ok(pulled_digest) => {
                if pulled_digest.0 != entry.image_digest {
                    errors.push(format!(
                        "digest mismatch for {}: expected {} got {}",
                        entry.tool_id,
                        short_digest(&entry.image_digest),
                        short_digest(&pulled_digest.0),
                    ));
                } else {
                    eprintln!(
                        "  {} {} {}  {}",
                        "Synced"
                            .if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
                        entry
                            .tool_id
                            .if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
                        entry.version,
                        short_digest(&entry.image_digest)
                            .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
                    );
                }
            }
            Err(e) => {
                errors.push(format!("failed to pull {}: {e}", entry.tool_id));
            }
        }
    }

    for warning in &drift_warnings {
        eprintln!(
            "  {} {warning}",
            "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
        );
    }

    if !errors.is_empty() {
        for err in &errors {
            eprintln!(
                "{} {err}",
                "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
            );
        }
        anyhow::bail!("sync failed");
    }

    Ok(())
}

/// Verify every tool in `bv.toml` has an entry in `lockfile` and vice versa.
fn check_frozen(bv_toml: &BvToml, lockfile: &bv_core::lockfile::Lockfile) -> anyhow::Result<()> {
    let mut issues = Vec::new();

    let declared: HashSet<&str> = bv_toml.tools.iter().map(|t| t.id.as_str()).collect();
    let locked: HashSet<&str> = lockfile.tools.keys().map(|s| s.as_str()).collect();

    for id in &declared {
        if !locked.contains(*id) {
            issues.push(format!("  {} declared in bv.toml but not in bv.lock", id));
        }
    }
    for id in &locked {
        if !declared.contains(*id) {
            issues.push(format!("  {} in bv.lock but not declared in bv.toml", id));
        }
    }

    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("{issue}");
        }
        anyhow::bail!("bv.toml and bv.lock are out of sync; run `bv lock` to update");
    }
    Ok(())
}

/// Best-effort drift detection: compare lockfile manifest sha256 against
/// the current registry. Returns None if registry is unavailable.
fn check_drift(
    lockfile: &bv_core::lockfile::Lockfile,
    registry_flag: Option<&str>,
) -> Option<Vec<String>> {
    let registry_url = crate::registry::resolve_registry_url(registry_flag, None);

    let cache = bv_core::cache::CacheLayout::new();
    let index = crate::registry::open_index(&registry_url, &cache);

    // Suppress refresh errors; drift detection is best-effort.
    index.refresh_if_stale(crate::registry::STALE_TTL).ok()?;

    let mut warnings = Vec::new();
    for (tool_id, entry) in &lockfile.tools {
        if entry.manifest_sha256.is_empty() {
            continue;
        }
        let version_req: VersionReq = entry
            .version
            .parse::<semver::Version>()
            .ok()
            .and_then(|v| semver::VersionReq::parse(&format!("={v}")).ok())
            .unwrap_or(VersionReq::STAR);

        if let Ok(manifest) = index.get_manifest(tool_id, &version_req)
            && let Ok(current_sha256) = ops::compute_manifest_sha256(&manifest)
            && current_sha256 != entry.manifest_sha256
        {
            warnings.push(format!(
                "manifest for {}@{} has changed since lock; run `bv lock` to update",
                tool_id, entry.version
            ));
        }
    }
    Some(warnings)
}

/// Strip the tag from an image reference to get the base for digest lookups.
fn base_image_ref(image_reference: &str) -> String {
    if let Some(colon_pos) = image_reference.rfind(':') {
        let before = &image_reference[..colon_pos];
        if before.contains('/') || !before.contains(':') {
            return before.to_string();
        }
    }
    image_reference.to_string()
}
