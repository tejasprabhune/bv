use std::path::PathBuf;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::manifest::Manifest;
use bv_core::project::BvLock;
use bv_runtime::{ContainerRuntime, DockerRuntime, GpuProfile, Mount, OciRef, RunSpec};

pub fn run(tool: &str, args: &[String]) -> anyhow::Result<()> {
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
        anyhow::anyhow!(
            "Tool '{tool}' is not in this project. Run `bv add {tool}` first."
        )
    })?;

    // Load the cached manifest to get the entrypoint.
    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(tool, &entry.version);
    let manifest = if manifest_path.exists() {
        let s = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read cached manifest for '{tool}'"))?;
        Manifest::from_toml_str(&s)?
    } else {
        anyhow::bail!(
            "Cached manifest for '{tool}@{}' not found.\n\
             Try running `bv add {tool}` again to re-resolve.",
            entry.version
        );
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

    let spec = RunSpec {
        image,
        command,
        env: manifest.tool.entrypoint.env.clone(),
        mounts: vec![Mount {
            host_path: cwd.clone(),
            container_path: PathBuf::from("/workspace"),
            read_only: false,
        }],
        gpu: GpuProfile {
            spec: manifest.tool.hardware.gpu.clone(),
        },
        working_dir: Some(PathBuf::from("/workspace")),
    };

    let runtime = DockerRuntime;
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
    let entry = lockfile.tools.get(tool).ok_or_else(|| {
        anyhow::anyhow!("Tool '{tool}' is not in this project.")
    })?;

    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(tool, &entry.version);

    println!("Tool:      {}", entry.tool_id);
    println!("Version:   {}", entry.version);
    println!("Image:     {}", entry.image_reference);
    println!("Digest:    {}", entry.image_digest);
    if let Some(sz) = entry.image_size_bytes {
        println!("Size:      {}", crate::commands::add::format_size(sz));
    }
    println!("Locked:    {}", entry.resolved_at.format("%Y-%m-%d %H:%M UTC"));
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
        eprintln!("{}", "(manifest not cached; run `bv add` to refresh)".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()));
    }

    Ok(())
}
