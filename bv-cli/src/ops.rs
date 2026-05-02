use std::collections::BTreeMap;
use std::sync::Arc;

/// Resolve the concurrent-pull cap for `bv add`/`bv sync`.
///
/// Priority: explicit `--jobs N` flag > `BV_JOBS` env var (handled by clap) >
/// `min(8, num_cpus)`. The 8-cap is conservative; image pulls are network- and
/// disk-bound, so once both are saturated more parallelism just queues. Tune up
/// on machines with fast network and lots of cores.
pub fn default_jobs(explicit: Option<usize>) -> usize {
    explicit.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8)
    })
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
use bv_runtime::{ContainerRuntime, ImageDigest, ImageRef, LayerSpec, OciRef};

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
///
/// Dispatches to the factored pull path when the manifest has a `[tool.factored]`
/// section with a pinned digest; falls back to the legacy single-image pull otherwise.
pub fn pull_and_make_entry(
    resolved: &ResolvedTool,
    reporter: &CliProgressReporter,
    cache: &CacheLayout,
    runtime: &AnyRuntime,
) -> anyhow::Result<LockfileEntry> {
    let entry = if let Some(factored) = &resolved.manifest.tool.factored {
        if !factored.image_digest.is_empty() {
            pull_and_make_entry_factored(resolved, factored, reporter, cache, runtime)?
        } else {
            pull_and_make_entry_legacy(resolved, reporter, cache, runtime)?
        }
    } else {
        pull_and_make_entry_legacy(resolved, reporter, cache, runtime)?
    };
    let _ = bv_core::owned_images::record(
        &cache.owned_images_path(),
        &entry.image_reference,
        &entry.image_digest,
    );
    Ok(entry)
}

fn pull_and_make_entry_legacy(
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
        spec_kind: bv_core::lockfile::SpecKind::LegacyImage,
        image_reference: resolved.manifest.tool.image.reference.clone(),
        image_digest: digest.0,
        manifest_sha256: resolved.manifest_sha256.clone(),
        image_size_bytes: size_bytes,
        layers: vec![],
        resolved_at: Utc::now(),
        reference_data_pins: BTreeMap::new(),
        binaries,
    })
}

fn pull_and_make_entry_factored(
    resolved: &ResolvedTool,
    factored: &bv_core::manifest::FactoredSpec,
    reporter: &CliProgressReporter,
    cache: &CacheLayout,
    runtime: &AnyRuntime,
) -> anyhow::Result<LockfileEntry> {
    use bv_core::lockfile::{CondaPackagePin, LayerDescriptor};

    let tool_label = format!("{}@{}", resolved.tool_id, resolved.manifest.tool.version);
    let tool_label = tool_label.if_supports_color(Stream::Stderr, |t| t.bold().to_string());
    let pulling = "Pulling".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string());
    if factored.layers.is_empty() {
        reporter.println(&format!("  {pulling} {tool_label}"));
    } else {
        reporter.println(&format!(
            "  {pulling} {tool_label} ({} layers)",
            factored.layers.len()
        ));
    }

    let layer_specs: Vec<LayerSpec> = factored
        .layers
        .iter()
        .map(|l| LayerSpec {
            digest: l.digest.clone(),
            size: l.size,
            media_type: l.media_type.clone(),
            blob_url: None,
        })
        .collect();

    // Pin digest alongside the tag so Docker pulls exactly the locked version.
    let factored_ref_str = if factored.image_reference.contains('@') {
        factored.image_reference.clone()
    } else {
        format!("{}@{}", factored.image_reference, factored.image_digest)
    };
    let factored_oci_ref: OciRef = factored_ref_str.parse().map_err(|e| {
        anyhow::anyhow!(
            "invalid factored image reference '{}': {}",
            factored_ref_str,
            e
        )
    })?;

    runtime
        .ensure_layers(&layer_specs, reporter)
        .with_context(|| format!("failed to stage layers for '{}'", resolved.tool_id))?;

    let image_ref: ImageRef = runtime
        .assemble_image(&factored_oci_ref, &layer_specs, reporter)
        .with_context(|| {
            format!(
                "failed to assemble factored image for '{}'",
                resolved.tool_id
            )
        })?;

    let size_bytes = runtime
        .inspect(&ImageDigest(image_ref.digest.clone()))
        .ok()
        .and_then(|m| m.size_bytes);

    crate::commands::add::cache_manifest(cache, &resolved.manifest)?;

    let version_str = if resolved.version_req == VersionReq::STAR {
        String::new()
    } else {
        resolved.version_req.to_string()
    };

    let short = crate::commands::add::short_digest(&image_ref.digest);
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

    let layers: Vec<LayerDescriptor> = factored
        .layers
        .iter()
        .map(|l| LayerDescriptor {
            digest: l.digest.clone(),
            size: l.size,
            media_type: l.media_type.clone(),
            conda_package: l.conda_package.as_ref().map(|p| CondaPackagePin {
                name: p.name.clone(),
                version: p.version.clone(),
                build: p.build.clone(),
                channel: p.channel.clone(),
                sha256: p.sha256.clone(),
            }),
        })
        .collect();

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
        spec_kind: bv_core::lockfile::SpecKind::FactoredOci,
        image_reference: factored.image_reference.clone(),
        image_digest: image_ref.digest,
        manifest_sha256: resolved.manifest_sha256.clone(),
        image_size_bytes: size_bytes,
        layers,
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
///
/// For `factored_oci` entries, reports at layer granularity so that individual
/// conda-package layer changes are visible in the diff output.
pub fn lock_diff(old: &Lockfile, new: &Lockfile) -> Vec<String> {
    use bv_core::lockfile::SpecKind;

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
                } else if old_entry.image_digest != new_entry.image_digest {
                    let old_d = crate::commands::add::short_digest(&old_entry.image_digest);
                    let new_d = crate::commands::add::short_digest(&new_entry.image_digest);
                    lines.push(format!("  ~ {} digest {} -> {}", id, old_d, new_d));
                } else if matches!(new_entry.spec_kind, SpecKind::FactoredOci) {
                    // Per-layer diff for factored images.
                    for (i, (old_layer, new_layer)) in old_entry
                        .layers
                        .iter()
                        .zip(new_entry.layers.iter())
                        .enumerate()
                    {
                        if old_layer.digest != new_layer.digest {
                            let pkg_note = new_layer
                                .conda_package
                                .as_ref()
                                .map(|p| format!(" ({}@{})", p.name, p.version))
                                .unwrap_or_default();
                            let old_d = crate::commands::add::short_digest(&old_layer.digest);
                            let new_d = crate::commands::add::short_digest(&new_layer.digest);
                            lines.push(format!(
                                "  ~ {} layer[{i}]{pkg_note} digest {} -> {}",
                                id, old_d, new_d
                            ));
                        }
                    }
                    if new_entry.layers.len() != old_entry.layers.len() {
                        lines.push(format!(
                            "  ~ {} layer count {} -> {}",
                            id,
                            old_entry.layers.len(),
                            new_entry.layers.len()
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bv_core::lockfile::{CondaPackagePin, LayerDescriptor, Lockfile, LockfileEntry, SpecKind};
    use chrono::{DateTime, Utc};

    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn legacy_entry(id: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: id.into(),
            declared_version_req: "=1.0.0".into(),
            version: "1.0.0".into(),
            spec_kind: SpecKind::LegacyImage,
            image_reference: format!("registry/{id}:1.0.0"),
            image_digest: format!("sha256:img-{id}"),
            manifest_sha256: format!("sha256:man-{id}"),
            image_size_bytes: None,
            layers: vec![],
            resolved_at: ts(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![id.into()],
        }
    }

    fn factored_entry(id: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: id.into(),
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
                    conda_package: Some(CondaPackagePin {
                        name: id.into(),
                        version: "1.0.0".into(),
                        build: "h0_0".into(),
                        channel: "bioconda".into(),
                        sha256: "efgh".into(),
                    }),
                },
            ],
            resolved_at: ts(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![id.into()],
        }
    }

    fn lock_with(entries: Vec<LockfileEntry>) -> Lockfile {
        let mut lock = Lockfile::new();
        for e in entries {
            lock.tools.insert(e.tool_id.clone(), e);
        }
        lock
    }

    #[test]
    fn lock_diff_no_changes_returns_empty() {
        let old = lock_with(vec![legacy_entry("blast"), factored_entry("samtools")]);
        let new = old.clone();
        assert!(lock_diff(&old, &new).is_empty());
    }

    #[test]
    fn lock_diff_detects_new_tool() {
        let old = lock_with(vec![legacy_entry("blast")]);
        let new = lock_with(vec![legacy_entry("blast"), legacy_entry("bwa")]);
        let diff = lock_diff(&old, &new);
        assert!(diff.iter().any(|l| l.contains("bwa") && l.contains("new")));
    }

    #[test]
    fn lock_diff_detects_removed_tool() {
        let old = lock_with(vec![legacy_entry("blast"), legacy_entry("bwa")]);
        let new = lock_with(vec![legacy_entry("blast")]);
        let diff = lock_diff(&old, &new);
        assert!(
            diff.iter()
                .any(|l| l.contains("bwa") && l.contains("removed"))
        );
    }

    #[test]
    fn lock_diff_detects_image_digest_change() {
        let old = lock_with(vec![legacy_entry("blast")]);
        let mut new_entry = legacy_entry("blast");
        new_entry.image_digest = "sha256:different".into();
        let new = lock_with(vec![new_entry]);
        let diff = lock_diff(&old, &new);
        assert!(
            diff.iter()
                .any(|l| l.contains("blast") && l.contains("digest"))
        );
    }

    #[test]
    fn lock_diff_detects_image_change_for_factored() {
        let old = lock_with(vec![factored_entry("samtools")]);
        let mut new_entry = factored_entry("samtools");
        new_entry.layers[0].digest = "sha256:openssl-upgraded".into();
        new_entry.image_digest = "sha256:img-samtools-new".into();
        let new = lock_with(vec![new_entry]);
        let diff = lock_diff(&old, &new);
        assert!(diff.iter().any(|l| l.contains("samtools")));
    }

    #[test]
    fn lock_diff_detects_version_change() {
        let old = lock_with(vec![legacy_entry("blast")]);
        let mut new_entry = legacy_entry("blast");
        new_entry.version = "2.0.0".into();
        let new = lock_with(vec![new_entry]);
        let diff = lock_diff(&old, &new);
        assert!(
            diff.iter()
                .any(|l| l.contains("blast") && l.contains("version"))
        );
    }

    #[test]
    fn lock_check_exits_nonzero_semantics_with_layers() {
        let old = lock_with(vec![factored_entry("samtools")]);
        let same = old.clone();
        assert!(
            old.is_equivalent_to(&same),
            "identical locks must be equivalent"
        );

        let mut changed = old.clone();
        changed.tools.get_mut("samtools").unwrap().layers[0].digest = "sha256:tampered".into();
        assert!(
            !old.is_equivalent_to(&changed),
            "layer change must break equivalence"
        );
    }
}
