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

    let path: Box<dyn InstallPath> = if cli.apptainer {
        Box::new(ApptainerPath)
    } else {
        Box::new(DockerPath)
    };

    let results = run_suite(path.as_ref(), &fixtures, &flags, &cli.work_dir);
    let report = BenchReport::new(results);

    if let Some(out) = &cli.json_out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out, json)?;
    } else {
        report.print_table();
    }
    Ok(())
}

struct DockerPath;

impl InstallPath for DockerPath {
    fn name(&self) -> &str {
        "docker-legacy"
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
