use std::collections::HashMap;
use std::fs;
use std::io::Write as _;

use anyhow::Context;
use indicatif::MultiProgress;
use owo_colors::{OwoColorize, Stream};
use semver::{Version, VersionReq};

use bv_core::cache::CacheLayout;
use bv_core::hardware::DetectedHardware;
use bv_core::lockfile::Lockfile;
use bv_core::manifest::Manifest;
use bv_core::project::{BvLock, BvToml, ProjectMeta, RegistryConfig, ToolDeclaration};
use bv_index::IndexBackend as _;
use bv_runtime::{ContainerRuntime, DockerRuntime, OciRef};

use crate::errors::print_hardware_mismatch;
use crate::ops;
use crate::progress::CliProgressReporter;

pub async fn run(
    tool_specs: &[String],
    registry_flag: Option<&str>,
    ignore_hardware: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    let mut bv_toml = if bv_toml_path.exists() {
        BvToml::from_path(&bv_toml_path).context("failed to read bv.toml")?
    } else {
        let name = cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        eprintln!(
            "  {} no bv.toml found, creating one",
            "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        BvToml {
            project: ProjectMeta {
                name,
                description: None,
            },
            registry: None,
            tools: vec![],
            data: HashMap::new(),
            hardware: Default::default(),
        }
    };

    let mut lockfile = if bv_lock_path.exists() {
        BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?
    } else {
        Lockfile::new()
    };

    let registry_url = crate::registry::resolve_registry_url(registry_flag, Some(&bv_toml));

    if bv_toml.registry.is_none() {
        bv_toml.registry = Some(RegistryConfig {
            url: registry_url.clone(),
        });
    }

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

    // Resolve manifests for each spec.
    let mut to_add: Vec<ops::ResolvedTool> = Vec::new();

    for spec in tool_specs {
        let (tool_id, version_req) = parse_tool_spec(spec)?;

        let manifest = index
            .get_manifest(&tool_id, &version_req)
            .with_context(|| format!("could not resolve '{}' in registry", spec))?;
        manifest.validate().map_err(|e| {
            anyhow::anyhow!("manifest validation errors for '{}': {:?}", tool_id, e)
        })?;

        if let Some(existing) = lockfile.tools.get(&tool_id)
            && existing.version == manifest.tool.version
        {
            eprintln!(
                "  {} {} {} is already up to date",
                "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
                tool_id,
                manifest
                    .tool
                    .version
                    .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
            );
            continue;
        }

        let oci_ref: OciRef = manifest
            .tool
            .image
            .reference
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid image ref for '{}': {}", tool_id, e))?;

        let manifest_sha256 = ops::compute_manifest_sha256(&manifest)?;

        to_add.push(ops::ResolvedTool {
            tool_id,
            version_req,
            manifest,
            oci_ref,
            manifest_sha256,
        });
    }

    if to_add.is_empty() {
        return Ok(());
    }

    // Hardware check all tools before pulling.
    if !ignore_hardware {
        let hw = DetectedHardware::detect();
        let mut any_mismatch = false;
        for r in &to_add {
            let mismatches = r.manifest.tool.hardware.check_against(&hw);
            if !mismatches.is_empty() {
                print_hardware_mismatch(&r.tool_id, &r.manifest.tool.version, &mismatches);
                any_mismatch = true;
            }
        }
        if any_mismatch {
            anyhow::bail!("hardware requirements not met; use --ignore-hardware to override");
        }
    } else if to_add.iter().any(|r| {
        r.manifest
            .tool
            .hardware
            .gpu
            .as_ref()
            .is_some_and(|g| g.required)
    }) {
        eprintln!(
            "  {} ignoring hardware requirements for GPU tool(s)",
            "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
        );
    }

    // Warn about required reference data before pulling images.
    for r in &to_add {
        if !r.manifest.tool.reference_data.is_empty() {
            print_reference_data_notice(&r.tool_id, &r.manifest);
        }
    }

    DockerRuntime
        .health_check()
        .context("Docker is not available. Is Docker Desktop running?")?;

    // Pull all tools (shared with bv lock/sync via ops).
    let mp = MultiProgress::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    let mut join_set: tokio::task::JoinSet<anyhow::Result<bv_core::lockfile::LockfileEntry>> =
        tokio::task::JoinSet::new();

    for r in to_add {
        let existing = lockfile.tools.get(&r.tool_id).cloned();
        let reporter = CliProgressReporter::for_multi(&mp);
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        join_set.spawn_blocking(move || {
            let _permit = permit;
            ops::pull_or_reuse(r, existing.as_ref(), &reporter)
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

    // Update bv.toml and bv.lock atomically.
    for entry in &pulled {
        if !bv_toml.tools.iter().any(|t| t.id == entry.tool_id) {
            let version_str = entry.declared_version_req.clone();
            bv_toml.tools.push(ToolDeclaration {
                id: entry.tool_id.clone(),
                version: version_str,
            });
        }
        lockfile.tools.insert(entry.tool_id.clone(), entry.clone());
    }

    bv_toml.to_path(&bv_toml_path)?;
    BvLock::to_path(&lockfile, &bv_lock_path)?;

    Ok(())
}

fn print_reference_data_notice(tool_id: &str, manifest: &Manifest) {
    let mut datasets: Vec<_> = manifest.tool.reference_data.values().collect();
    datasets.sort_by(|a, b| a.id.cmp(&b.id));

    eprintln!(
        "\n  {} requires the following reference datasets:",
        tool_id.if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );

    let mut fetch_ids: Vec<String> = Vec::new();
    for spec in &datasets {
        let size_hint = spec
            .size_bytes
            .map(|b| {
                format!(
                    "  {}",
                    format_size(b).if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
                )
            })
            .unwrap_or_default();
        let req_tag: String = if spec.required {
            "required"
                .if_supports_color(Stream::Stderr, |t| t.yellow().to_string())
                .to_string()
        } else {
            "optional"
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
                .to_string()
        };
        eprintln!(
            "    {}@{}{}  ({})",
            spec.id, spec.version, size_hint, req_tag
        );
        fetch_ids.push(spec.id.clone());
    }

    eprintln!(
        "\n  Fetch with: {}",
        format!("bv data fetch {}", fetch_ids.join(" "))
            .if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
}

pub fn parse_tool_spec(spec: &str) -> anyhow::Result<(String, VersionReq)> {
    if let Some((id, ver_str)) = spec.split_once('@') {
        let req = if ver_str == "latest" {
            VersionReq::STAR
        } else if ver_str.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            ver_str.parse::<Version>().map_err(|_| {
                anyhow::anyhow!(
                    "'{}' is not a valid semver version. Use @2.15.0, @^2.15, or @>=2.14",
                    ver_str
                )
            })?;
            VersionReq::parse(&format!("={}", ver_str))
                .map_err(|e| anyhow::anyhow!("invalid version req: {}", e))?
        } else {
            VersionReq::parse(ver_str)
                .map_err(|e| anyhow::anyhow!("invalid version requirement '{}': {}", ver_str, e))?
        };
        Ok((id.to_string(), req))
    } else {
        Ok((spec.to_string(), VersionReq::STAR))
    }
}

pub fn cache_manifest(cache: &CacheLayout, manifest: &Manifest) -> anyhow::Result<()> {
    let path = cache.manifest_path(&manifest.tool.id, &manifest.tool.version);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, manifest.to_toml_string()?)?;
    Ok(())
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn short_digest(digest: &str) -> &str {
    digest
        .find(':')
        .and_then(|i| digest.get(i + 1..i + 13))
        .unwrap_or(digest)
}
