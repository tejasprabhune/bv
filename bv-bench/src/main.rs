use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use bv_bench::{
    fixture::Fixture,
    harness::{BenchFlags, InstallPath, run_suite},
    report::BenchReport,
};
use clap::Parser;

#[derive(Parser)]
#[command(name = "bv-bench", about = "bv install-path benchmark harness")]
struct Cli {
    /// Run only against the fixtures with this many tools (1, 5, or 20).
    #[arg(long, value_delimiter = ',')]
    tools: Option<Vec<usize>>,

    /// Restrict to Linux Docker (skip on macOS).
    #[arg(long)]
    linux_only: bool,

    /// Exercise the Apptainer backend instead of Docker.
    #[arg(long)]
    apptainer: bool,

    /// Also benchmark mamba install path.
    #[arg(long)]
    mamba: bool,

    /// Also benchmark pixi install path.
    #[arg(long)]
    pixi: bool,

    /// Write JSON results to this file instead of stdout.
    #[arg(long)]
    json_out: Option<PathBuf>,

    /// Working directory root for per-fixture scratch dirs.
    #[arg(long, default_value = "/tmp/bv-bench")]
    work_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let flags = BenchFlags {
        linux_only: cli.linux_only,
        apptainer: cli.apptainer,
    };

    let mut fixtures = Fixture::standard_suite();
    if let Some(sizes) = &cli.tools {
        fixtures.retain(|f| sizes.contains(&f.tools.len()));
    }

    let mut paths: Vec<Box<dyn InstallPath>> = Vec::new();
    if cli.apptainer {
        paths.push(Box::new(ApptainerPath));
    } else {
        paths.push(Box::new(BvPath));
    }
    if cli.mamba {
        paths.push(Box::new(MambaPath));
    }
    if cli.pixi {
        paths.push(Box::new(PixiPath));
    }

    let mut all_results = Vec::new();
    for path in &paths {
        let path_work = cli.work_dir.join(path.name());
        std::fs::create_dir_all(&path_work)?;
        let results = run_suite(path.as_ref(), &fixtures, &flags, &path_work);
        all_results.extend(results);
    }

    let report = BenchReport::new(all_results);

    if let Some(out) = &cli.json_out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out, json)?;
    } else {
        report.print_table();
    }
    Ok(())
}

struct BvPath;

impl InstallPath for BvPath {
    fn name(&self) -> &str {
        "bv"
    }

    fn install(
        &self,
        fixture: &Fixture,
        work_dir: &std::path::Path,
    ) -> Result<(u64, Duration)> {
        let bv = find_bv()?;
        let registry = std::env::var("BV_REGISTRY").unwrap_or_default();
        for tool in &fixture.tools {
            let mut cmd = std::process::Command::new(&bv);
            cmd.arg("add").arg(format!("{}@{}", tool.id, tool.version));
            if !registry.is_empty() {
                cmd.arg("--registry").arg(&registry);
            }
            cmd.current_dir(work_dir);
            let status = cmd.status()?;
            if !status.success() {
                bail!("bv add {} failed", tool.id);
            }
        }
        let footprint = dir_size(work_dir)?;
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let bv = find_bv()?;
        let tool = fixture.tools.first().expect("non-empty fixture");
        let start = std::time::Instant::now();
        let status = std::process::Command::new(&bv)
            .args(["run", &tool.id, "--", "--version"])
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} failed", tool.id);
        }
        Ok(elapsed)
    }
}

struct ApptainerPath;

impl InstallPath for ApptainerPath {
    fn name(&self) -> &str {
        "apptainer"
    }

    fn install(
        &self,
        fixture: &Fixture,
        work_dir: &std::path::Path,
    ) -> Result<(u64, Duration)> {
        let bv = find_bv()?;
        let registry = std::env::var("BV_REGISTRY").unwrap_or_default();
        for tool in &fixture.tools {
            let mut cmd = std::process::Command::new(&bv);
            cmd.args(["add", &format!("{}@{}", tool.id, tool.version), "--runtime", "apptainer"]);
            if !registry.is_empty() {
                cmd.arg("--registry").arg(&registry);
            }
            cmd.current_dir(work_dir);
            let status = cmd.status()?;
            if !status.success() {
                bail!("bv add {} (apptainer) failed", tool.id);
            }
        }
        let footprint = dir_size(work_dir)?;
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let bv = find_bv()?;
        let tool = fixture.tools.first().expect("non-empty fixture");
        let start = std::time::Instant::now();
        let status = std::process::Command::new(&bv)
            .args(["run", &tool.id, "--", "--version"])
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} (apptainer) failed", tool.id);
        }
        Ok(elapsed)
    }
}

struct MambaPath;

impl InstallPath for MambaPath {
    fn name(&self) -> &str {
        "mamba"
    }

    fn install(
        &self,
        fixture: &Fixture,
        work_dir: &std::path::Path,
    ) -> Result<(u64, Duration)> {
        let conda_root = conda_base()?;
        let mut total_bytes: u64 = 0;
        for tool in &fixture.tools {
            let env_name = mamba_env_name(fixture, &tool.id);
            let env_dir = conda_root.join("envs").join(&env_name);
            // Remove leftover env from a prior run.
            if env_dir.exists() {
                std::process::Command::new("mamba")
                    .args(["env", "remove", "-n", &env_name, "--yes"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()?;
            }
            let status = std::process::Command::new("mamba")
                .args([
                    "create", "-n", &env_name,
                    "-c", "bioconda", "-c", "conda-forge",
                    &format!("{}={}", tool.id, tool.version),
                    "--yes",
                ])
                .current_dir(work_dir)
                .status()?;
            if !status.success() {
                bail!("mamba create {} failed", tool.id);
            }
            total_bytes += dir_size(&env_dir).unwrap_or(0);
        }
        Ok((total_bytes, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let tool = fixture.tools.first().expect("non-empty fixture");
        let env_name = mamba_env_name(fixture, &tool.id);
        let start = std::time::Instant::now();
        let status = std::process::Command::new("conda")
            .args(["run", "--no-capture-output", "-n", &env_name, &tool.id, "--version"])
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} (mamba) failed", tool.id);
        }
        Ok(elapsed)
    }
}

struct PixiPath;

impl InstallPath for PixiPath {
    fn name(&self) -> &str {
        "pixi"
    }

    fn install(
        &self,
        fixture: &Fixture,
        work_dir: &std::path::Path,
    ) -> Result<(u64, Duration)> {
        // Init a fresh pixi project (or reuse existing pixi.toml).
        let pixi_toml = work_dir.join("pixi.toml");
        if !pixi_toml.exists() {
            let status = std::process::Command::new("pixi")
                .args(["init", "."])
                .current_dir(work_dir)
                .status()?;
            if !status.success() {
                bail!("pixi init failed");
            }
        }
        for tool in &fixture.tools {
            let status = std::process::Command::new("pixi")
                .args([
                    "add",
                    "--manifest-path", pixi_toml.to_str().unwrap(),
                    "-c", "bioconda",
                    "-c", "conda-forge",
                    &format!("{}=={}", tool.id, tool.version),
                ])
                .current_dir(work_dir)
                .status()?;
            if !status.success() {
                bail!("pixi add {} failed", tool.id);
            }
        }
        let footprint = dir_size(&work_dir.join(".pixi")).unwrap_or(0);
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let tool = fixture.tools.first().expect("non-empty fixture");
        let bin = work_dir.join(".pixi").join("envs").join("default").join("bin").join(&tool.id);
        let start = std::time::Instant::now();
        let status = std::process::Command::new(&bin)
            .arg("--version")
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} (pixi) failed", tool.id);
        }
        Ok(elapsed)
    }
}

fn find_bv() -> Result<PathBuf> {
    let candidate = std::env::current_exe()?
        .parent()
        .expect("exe has parent")
        .join("bv");
    if candidate.exists() {
        return Ok(candidate);
    }
    Ok(PathBuf::from("bv"))
}

fn dir_size(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path) {
        let entry: walkdir::DirEntry = entry?;
        if entry.file_type().is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

fn conda_base() -> Result<PathBuf> {
    let out = std::process::Command::new("conda")
        .args(["info", "--base"])
        .output()?;
    if !out.status.success() {
        bail!("conda info --base failed");
    }
    Ok(PathBuf::from(String::from_utf8(out.stdout)?.trim().to_string()))
}

fn mamba_env_name(fixture: &Fixture, tool_id: &str) -> String {
    format!("bv-bench-{}-{}", fixture.name, tool_id)
}
