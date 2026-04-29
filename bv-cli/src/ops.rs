use std::collections::BTreeMap;
use std::sync::Arc;

/// Resolve the concurrent-pull cap for `bv add`/`bv sync`.
///
/// Priority: explicit `--jobs N` flag > `BV_JOBS` env var (handled by clap) >
/// `min(8, num_cpus)`. The 8-cap is conservative; image pulls are network- and
/// disk-bound, so once both are saturated more parallelism just queues. Tune up
/// on machines with fast network and lots of cores.
pub fn default_jobs(explicit: Option<usize>) -> usize {
    explicit.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8))
}

use anyhow::Context;
use chrono::Utc;
use indicatif::MultiProgress;
use owo_colors::{OwoColorize, Stream};
use semver::VersionReq;
use sha2::{Digest, Sha256};

use bv_core::cache::CacheLayout;
use bv_core::lockfile::{Lockfile, LockfileEntry, LockfileMetadata};
use bv_core::manifest::Manifest;
use bv_index::{GitIndex, IndexBackend as _};
use bv_runtime::{ContainerRuntime, OciRef};

use crate::runtime_select::AnyRuntime;

use crate::commands::add::format_size;
use crate::progress::CliProgressReporter;

pub struct ResolvedTool {
    pub tool_id: String,
    pub version_req: VersionReq,
    pub manifest: Manifest,
    pub oci_ref: OciRef,
    pub manifest_sha256: String,
}

/// Compute the SHA-256 of a manifest's canonical TOML serialization.
pub fn compute_manifest_sha256(manifest: &Manifest) -> anyhow::Result<String> {
    let toml_str = manifest.to_toml_string()?;
    let mut hasher = Sha256::new();
    hasher.update(toml_str.as_bytes());
    let bytes = hasher.finalize();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}

/// Resolve every tool declared in `bv.toml` to a concrete manifest.
pub fn resolve_all(
    declared_tools: &[bv_core::project::ToolDeclaration],
    index: &GitIndex,
) -> anyhow::Result<Vec<ResolvedTool>> {
    let mut resolved = Vec::new();
    for decl in declared_tools {
        let version_req: VersionReq = if decl.version.is_empty() {
            VersionReq::STAR
        } else {
            decl.version
                .parse()
                .with_context(|| format!("invalid version req for '{}'", decl.id))?
        };
        let manifest = index
            .get_manifest(&decl.id, &version_req)
            .with_context(|| format!("could not resolve '{}' in registry", decl.id))?;
        manifest.validate().map_err(|e| {
            anyhow::anyhow!("manifest validation errors for '{}': {:?}", decl.id, e)
        })?;

        let oci_ref: OciRef = manifest
            .tool
            .image
            .reference
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid image ref for '{}': {}", decl.id, e))?;

        let manifest_sha256 = compute_manifest_sha256(&manifest)?;

        resolved.push(ResolvedTool {
            tool_id: decl.id.clone(),
            version_req,
            manifest,
            oci_ref,
            manifest_sha256,
        });
    }
    Ok(resolved)
}

/// Generate a complete lockfile from resolved tools.
///
/// Reuses entries from `existing` when the version and manifest sha256 match,
/// avoiding a redundant pull. Pulls new images in parallel (max 3 concurrent).
pub async fn generate_lockfile(
    resolved: Vec<ResolvedTool>,
    existing: Option<&Lockfile>,
    hardware_summary: Option<String>,
    mp: &MultiProgress,
    runtime: &AnyRuntime,
) -> anyhow::Result<Lockfile> {
    let mut new_lock = Lockfile {
        version: 1,
        metadata: LockfileMetadata {
            bv_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: Utc::now(),
            hardware_summary,
        },
        tools: BTreeMap::new(),
        binary_index: BTreeMap::new(),
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(3));
    let mut join_set: tokio::task::JoinSet<anyhow::Result<LockfileEntry>> =
        tokio::task::JoinSet::new();

    for r in resolved {
        let existing_entry = existing.and_then(|l| l.tools.get(&r.tool_id)).cloned();
        let reporter = CliProgressReporter::for_multi(mp);
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        let rt = runtime.clone();

        join_set.spawn_blocking(move || {
            let _permit = permit;
            pull_or_reuse(r, existing_entry.as_ref(), &reporter, &rt)
        });
    }

    while let Some(result) = join_set.join_next().await {
        let entry = result.context("pull task panicked")??;
        new_lock.tools.insert(entry.tool_id.clone(), entry);
    }

    Ok(new_lock)
}

/// Return a lockfile entry, reusing the existing one when the resolved state
/// is unchanged, pulling otherwise.
pub fn pull_or_reuse(
    resolved: ResolvedTool,
    existing: Option<&LockfileEntry>,
    reporter: &CliProgressReporter,
    runtime: &AnyRuntime,
) -> anyhow::Result<LockfileEntry> {
    if let Some(e) = existing {
        let version_matches = e.version == resolved.manifest.tool.version;
        let manifest_matches =
            e.manifest_sha256.is_empty() || e.manifest_sha256 == resolved.manifest_sha256;

        if version_matches && manifest_matches {
            let binaries = resolved
                .manifest
                .tool
                .effective_binaries()
                .into_iter()
                .map(str::to_string)
                .collect();
            return Ok(LockfileEntry {
                manifest_sha256: resolved.manifest_sha256,
                binaries,
                ..e.clone()
            });
        }
    }

    let cache = CacheLayout::new();
    pull_and_make_entry(&resolved, reporter, &cache, runtime)
}

/// Pull an image and build a fully-populated `LockfileEntry`.
pub fn pull_and_make_entry(
    resolved: &ResolvedTool,
    reporter: &CliProgressReporter,
    cache: &CacheLayout,
    runtime: &AnyRuntime,
) -> anyhow::Result<LockfileEntry> {
    reporter.println(&format!(
        "  {} {}",
        "Pulling".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        format!("{}@{}", resolved.tool_id, resolved.manifest.tool.version)
            .if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
    ));

    let digest = runtime
        .pull(&resolved.oci_ref, reporter)
        .with_context(|| format!("failed to pull '{}'", resolved.oci_ref.docker_arg()))?;

    let size_bytes = runtime.inspect(&digest).ok().and_then(|m| m.size_bytes);

    crate::commands::add::cache_manifest(cache, &resolved.manifest)?;

    let version_str = if resolved.version_req == VersionReq::STAR {
        String::new()
    } else {
        resolved.version_req.to_string()
    };

    let short = crate::commands::add::short_digest(&digest.0);
    let size_str = size_bytes.map(format_size).unwrap_or_default();
    reporter.println(&format!(
        "  {} {} {}  {} {}",
        "Added".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        resolved
            .tool_id
            .if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
        resolved.manifest.tool.version,
        short.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        size_str.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
    ));

    let binaries = resolved
        .manifest
        .tool
        .effective_binaries()
        .into_iter()
        .map(str::to_string)
        .collect();

    Ok(LockfileEntry {
        tool_id: resolved.tool_id.clone(),
        declared_version_req: version_str,
        version: resolved.manifest.tool.version.clone(),
        image_reference: resolved.manifest.tool.image.reference.clone(),
        image_digest: digest.0,
        manifest_sha256: resolved.manifest_sha256.clone(),
        image_size_bytes: size_bytes,
        resolved_at: Utc::now(),
        reference_data_pins: BTreeMap::new(),
        binaries,
    })
}

/// Strip the tag/digest from an image reference to get the base form.
/// e.g. `ncbi/blast:2.15.0` -> `ncbi/blast`, `ghcr.io/foo/bar:latest` -> `ghcr.io/foo/bar`
pub fn base_image_ref(image_reference: &str) -> String {
    if let Some(colon_pos) = image_reference.rfind(':') {
        let before = &image_reference[..colon_pos];
        if before.contains('/') || !before.contains(':') {
            return before.to_string();
        }
    }
    image_reference.to_string()
}

/// Describe the differences between two lockfiles for `bv lock --check`.
pub fn lock_diff(old: &Lockfile, new: &Lockfile) -> Vec<String> {
    let mut lines = Vec::new();

    for id in new.tools.keys() {
        if !old.tools.contains_key(id) {
            lines.push(format!("  + {} (new)", id));
        }
    }
    for (id, old_entry) in &old.tools {
        match new.tools.get(id) {
            None => lines.push(format!("  - {} (removed)", id)),
            Some(new_entry) if !old_entry.is_equivalent(new_entry) => {
                if old_entry.version != new_entry.version {
                    lines.push(format!(
                        "  ~ {} version {} -> {}",
                        id, old_entry.version, new_entry.version
                    ));
                } else {
                    let old_d = crate::commands::add::short_digest(&old_entry.image_digest);
                    let new_d = crate::commands::add::short_digest(&new_entry.image_digest);
                    lines.push(format!("  ~ {} digest {} -> {}", id, old_d, new_d));
                }
            }
            _ => {}
        }
    }
    lines
}
