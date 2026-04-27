use std::path::PathBuf;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::manifest::Manifest;
use bv_core::project::BvLock;
use bv_runtime::{ContainerRuntime, GpuProfile, Mount, OciRef, RunSpec};

pub async fn run(tool: &str, args: &[String], backend: Option<&str>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!(
            "No bv.lock found in this directory.\n\
             Run `bv add {tool}` first."
        );
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    let entry = lockfile.tools.get(tool).ok_or_else(|| {
        anyhow::anyhow!("Tool '{tool}' is not in this project. Run `bv add {tool}` first.")
    })?;

    // Load the cached manifest to get the entrypoint.
    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(tool, &entry.version);
    let manifest = if manifest_path.exists() {
        let s = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read cached manifest for '{tool}'"))?;
        Manifest::from_toml_str(&s)?
    } else {
        // Manifest was evicted from cache (e.g. user deleted ~/.cache/bv).
        // Recover it from the local git index clone without a network request.
        let index_manifest = cache
            .index_dir("default")
            .join("tools")
            .join(tool)
            .join(format!("{}.toml", entry.version));

        if index_manifest.exists() {
            let s = std::fs::read_to_string(&index_manifest)
                .with_context(|| format!("failed to read index manifest for '{tool}'"))?;
            let m = Manifest::from_toml_str(&s)?;
            crate::commands::add::cache_manifest(&cache, &m)
                .with_context(|| format!("failed to restore cached manifest for '{tool}'"))?;
            m
        } else {
            anyhow::bail!(
                "Manifest for '{tool}@{}' is not in cache or local index.\n\
                 Run `bv add {tool}` to restore it.",
                entry.version
            );
        }
    };

    // Build the OciRef with pinned digest for reproducible execution.
    let mut image: OciRef = entry
        .image_reference
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid image ref in lockfile: {e}"))?;
    image.tag = None;
    image.digest = Some(entry.image_digest.clone());

    // Decide command: user-supplied args override the manifest entrypoint.
    let command = if args.is_empty() {
        vec![manifest.tool.entrypoint.command.clone()]
    } else {
        args.to_vec()
    };

    // Build reference data mounts; fail fast on missing required datasets.
    let mut mounts = vec![Mount {
        host_path: cwd.clone(),
        container_path: PathBuf::from("/workspace"),
        read_only: false,
    }];
    let mut missing_required: Vec<String> = Vec::new();
    for (key, spec) in &manifest.tool.reference_data {
        let data_dir = cache.data_dir(&spec.id, &spec.version);
        if data_dir.exists() {
            if let Some(mount_path) = &spec.mount_path {
                mounts.push(Mount {
                    host_path: data_dir,
                    container_path: PathBuf::from(mount_path),
                    read_only: true,
                });
            }
        } else if spec.required {
            missing_required.push(format!("{}@{}", spec.id, spec.version));
            eprintln!(
                "  {} reference data '{key}' not found in cache",
                "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
            );
        } else {
            eprintln!(
                "  {} optional reference data '{key}' not downloaded; skipping",
                "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
            );
        }
    }
    if !missing_required.is_empty() {
        let fetch_args = missing_required
            .iter()
            .map(|s| s.split('@').next().unwrap_or(s).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        anyhow::bail!(
            "required reference data missing\n  \
             Run: bv data fetch {fetch_args}"
        );
    }

    let spec = RunSpec {
        image,
        command,
        env: manifest.tool.entrypoint.env.clone(),
        mounts,
        gpu: GpuProfile {
            spec: manifest.tool.hardware.gpu.clone(),
        },
        working_dir: Some(PathBuf::from("/workspace")),
    };

    let bv_toml = bv_core::project::BvToml::from_path(&cwd.join("bv.toml")).ok();
    let runtime = crate::runtime_select::resolve_runtime(backend, bv_toml.as_ref())?;

    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    // Check the pinned image is locally present; if not, guide the user to bv sync.
    let base_ref = crate::ops::base_image_ref(&entry.image_reference);
    if !runtime.is_locally_available(&base_ref, &entry.image_digest) {
        anyhow::bail!(
            "Image for '{tool}@{}' is not available locally.\n  \
             Run `bv sync` to pull it.",
            entry.version
        );
    }

    let outcome = runtime
        .run(&spec)
        .with_context(|| format!("failed to run '{tool}'"))?;

    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }

    Ok(())
}

pub fn info(tool: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!("No bv.lock found. Run `bv add {tool}` first.");
    }

    let lockfile = BvLock::from_path(&bv_lock_path)?;
    let entry = lockfile
        .tools
        .get(tool)
        .ok_or_else(|| anyhow::anyhow!("Tool '{tool}' is not in this project."))?;

    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(tool, &entry.version);

    println!("Tool:      {}", entry.tool_id);
    println!("Version:   {}", entry.version);
    println!("Image:     {}", entry.image_reference);
    println!("Digest:    {}", entry.image_digest);
    if let Some(sz) = entry.image_size_bytes {
        println!("Size:      {}", crate::commands::add::format_size(sz));
    }
    println!(
        "Locked:    {}",
        entry.resolved_at.format("%Y-%m-%d %H:%M UTC")
    );
    println!("Manifest:  {}", manifest_path.display());

    if manifest_path.exists() {
        let s = std::fs::read_to_string(&manifest_path)?;
        if let Ok(m) = Manifest::from_toml_str(&s) {
            if let Some(desc) = &m.tool.description {
                println!("About:     {desc}");
            }
            if let Some(hp) = &m.tool.homepage {
                println!("Homepage:  {hp}");
            }
            let ep = &m.tool.entrypoint;
            println!("Entrypoint: {}", ep.command);
            if let Some(tmpl) = &ep.args_template {
                println!("Args template: {tmpl}");
            }
        }
    } else {
        eprintln!(
            "{}",
            "(manifest not cached; run `bv add` to refresh)"
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
    }

    Ok(())
}
