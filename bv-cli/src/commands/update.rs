use std::io::Write as _;

use anyhow::Context;
use indicatif::MultiProgress;
use owo_colors::{OwoColorize, Stream};
use semver::VersionReq;

use bv_core::cache::CacheLayout;
use bv_core::hardware::DetectedHardware;
use bv_core::lockfile::Lockfile;
use bv_core::manifest::Tier;
use bv_core::project::{BvLock, BvToml};
use bv_index::IndexBackend as _;
use bv_runtime::ContainerRuntime as _;

use crate::errors::print_hardware_mismatch;
use crate::ops;
use crate::progress::CliProgressReporter;

pub async fn run(
    tools: &[String],
    registry_flag: Option<&str>,
    ignore_hardware: bool,
    backend_flag: Option<&str>,
    jobs: Option<usize>,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_toml_path.exists() {
        anyhow::bail!("no bv.toml found; run `bv add <tool>` to set up a project first");
    }

    let bv_toml = BvToml::from_path(&bv_toml_path).context("failed to read bv.toml")?;

    if bv_toml.tools.is_empty() {
        eprintln!(
            "  {} no tools declared in bv.toml",
            "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        return Ok(());
    }

    let tool_ids: Vec<String> = if tools.is_empty() {
        bv_toml.tools.iter().map(|t| t.id.clone()).collect()
    } else {
        for t in tools {
            if !bv_toml.tools.iter().any(|d| d.id == *t) {
                anyhow::bail!(
                    "'{}' is not in bv.toml; use `bv add {}` to install it first",
                    t,
                    t
                );
            }
        }
        tools.to_vec()
    };

    let mut lockfile = if bv_lock_path.exists() {
        BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?
    } else {
        Lockfile::new()
    };

    let registry_url = crate::registry::resolve_registry_url(registry_flag, Some(&bv_toml));
    let cache = CacheLayout::new();
    let index = crate::registry::open_index(&registry_url, &cache);

    eprint!(
        "  {} index",
        "Updating".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string())
    );
    std::io::stderr().flush().ok();
    index
        .refresh()
        .with_context(|| format!("registry refresh failed for '{}'", registry_url))?;
    eprintln!(
        " {}",
        "done".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
    );

    let mut to_pull: Vec<ops::ResolvedTool> = Vec::new();
    let mut already_latest: Vec<String> = Vec::new();

    for tool_id in &tool_ids {
        let manifest = index
            .get_manifest(tool_id, &VersionReq::STAR)
            .with_context(|| format!("could not resolve '{}' in registry", tool_id))?;
        manifest.validate().map_err(|e| {
            anyhow::anyhow!("manifest validation errors for '{}': {:?}", tool_id, e)
        })?;

        if manifest.tool.tier == Tier::Experimental {
            eprintln!(
                "  {} '{}' is marked experimental; skipping (use `bv add {} --allow-experimental` to install)",
                "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
                tool_id,
                tool_id,
            );
            continue;
        }

        if manifest.tool.deprecated {
            eprintln!(
                "  {} '{}' is deprecated; consider looking for a maintained alternative",
                "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
                tool_id
            );
        }

        if let Some(existing) = lockfile.tools.get(tool_id.as_str())
            && existing.version == manifest.tool.version
        {
            already_latest.push(format!("{} {}", tool_id, existing.version));
            continue;
        }

        let oci_ref = manifest
            .tool
            .image
            .reference
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid image ref for '{}': {}", tool_id, e))?;

        let manifest_sha256 = ops::compute_manifest_sha256(&manifest)?;

        // Pin the lock entry to the exact resolved version so bv.lock never
        // records "*" -- the lock should always reflect what was actually pulled.
        let exact_req = VersionReq::parse(&format!("={}", manifest.tool.version))
            .expect("manifest version is valid semver");

        to_pull.push(ops::ResolvedTool {
            tool_id: tool_id.clone(),
            version_req: exact_req,
            manifest,
            oci_ref,
            manifest_sha256,
        });
    }

    for label in &already_latest {
        eprintln!(
            "  {} {} (already latest)",
            "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
            label,
        );
    }

    if to_pull.is_empty() {
        return Ok(());
    }

    if !ignore_hardware {
        let hw = DetectedHardware::detect();
        let mut any_mismatch = false;
        for r in &to_pull {
            let mismatches = r.manifest.tool.hardware.check_against(&hw);
            if !mismatches.is_empty() {
                print_hardware_mismatch(&r.tool_id, &r.manifest.tool.version, &mismatches);
                any_mismatch = true;
            }
        }
        if any_mismatch {
            anyhow::bail!("hardware requirements not met; use --ignore-hardware to override");
        }
    }

    let runtime = crate::runtime_select::resolve_runtime(backend_flag, Some(&bv_toml))?;
    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    let mp = MultiProgress::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(ops::default_jobs(jobs)));
    let mut join_set: tokio::task::JoinSet<anyhow::Result<bv_core::lockfile::LockfileEntry>> =
        tokio::task::JoinSet::new();

    for r in to_pull {
        let existing = lockfile.tools.get(&r.tool_id).cloned();
        let reporter = CliProgressReporter::for_multi(&mp);
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        let rt = runtime.clone();
        join_set.spawn_blocking(move || {
            let _permit = permit;
            ops::pull_or_reuse(r, existing.as_ref(), &reporter, &rt)
        });
    }

    let mut pulled: Vec<bv_core::lockfile::LockfileEntry> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(entry)) => pulled.push(entry),
            Ok(Err(e)) => failures.push(e.to_string()),
            Err(e) => failures.push(format!("task panicked: {e}")),
        }
    }

    if !failures.is_empty() {
        for f in &failures {
            eprintln!(
                "{} {}",
                "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string()),
                f
            );
        }
        anyhow::bail!("one or more pulls failed; lockfile not updated");
    }

    for entry in &pulled {
        lockfile.tools.insert(entry.tool_id.clone(), entry.clone());
    }

    lockfile
        .rebuild_binary_index(&bv_toml.binary_overrides)
        .map_err(|e| {
            anyhow::anyhow!(
                "binary name collision: {e}\n  \
                 Add [binary_overrides] to bv.toml to resolve, e.g.:\n  \
                 [binary_overrides]\n  \
                 <binary> = \"<tool-id>\""
            )
        })?;

    BvLock::to_path(&lockfile, &bv_lock_path)?;
    crate::shims::write_shims(&cwd, &lockfile)?;

    Ok(())
}
