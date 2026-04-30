use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use bv_bench::{
    fixture::Fixture,
    harness::{BenchFlags, InstallPath, run_suite},
    report::BenchReport,
};
use clap::Parser;

#[derive(clap::ValueEnum, Clone, Debug)]
enum Suite {
    /// All tools available on osx-arm64; mamba/pixi/conda succeed on macOS.
    Mac,
    /// Includes Linux-only tools; mamba/pixi/conda will fail some fixtures on macOS.
    Linux,
}

#[derive(Parser)]
#[command(name = "bv-bench", about = "bv install-path benchmark harness")]
struct Cli {
    #[arg(long, default_value = "mac")]
    suite: Suite,

    #[arg(long)]
    linux_only: bool,

    #[arg(long)]
    apptainer: bool,

    #[arg(long)]
    mamba: bool,

    #[arg(long)]
    micromamba: bool,

    #[arg(long)]
    pixi: bool,

    #[arg(long)]
    conda: bool,

    #[arg(long)]
    json_out: Option<PathBuf>,

    #[arg(long, default_value = "/tmp/bv-bench")]
    work_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let flags = BenchFlags {
        linux_only: cli.linux_only,
        apptainer: cli.apptainer,
    };

    let fixtures = match cli.suite {
        Suite::Mac => Fixture::mac_suite(),
        Suite::Linux => Fixture::linux_suite(),
    };

    let mut paths: Vec<Box<dyn InstallPath>> = Vec::new();
    if cli.apptainer {
        paths.push(Box::new(ApptainerPath));
    } else {
        paths.push(Box::new(BvPath));
    }
    if cli.mamba {
        match find_conda_like("mamba") {
            Ok(bin) => paths.push(Box::new(CondaLikePath { name: "mamba".into(), bin })),
            Err(e) => eprintln!("warning: skipping mamba ({})", e),
        }
    }
    if cli.micromamba {
        match find_conda_like("micromamba") {
            Ok(bin) => paths.push(Box::new(MicromambaPath { bin })),
            Err(e) => eprintln!("warning: skipping micromamba ({})", e),
        }
    }
    if cli.conda {
        match find_conda_like("conda") {
            Ok(bin) => paths.push(Box::new(CondaLikePath { name: "conda".into(), bin })),
            Err(e) => eprintln!("warning: skipping conda ({})", e),
        }
    }
    if cli.pixi {
        match which::which("pixi") {
            Ok(_) => paths.push(Box::new(PixiPath)),
            Err(_) => eprintln!("warning: skipping pixi (not found on PATH)"),
        }
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

    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)> {
        let bv = find_bv()?;
        let registry = std::env::var("BV_REGISTRY").unwrap_or_default();

        // Start clean: fresh work_dir so bv add resolves and writes a new lock.
        if work_dir.exists() {
            std::fs::remove_dir_all(work_dir)?;
        }
        std::fs::create_dir_all(work_dir)?;

        // Register all tools in one pass (single index refresh + parallel pull).
        // Any images already in Docker cache are reused; new ones are pulled in parallel.
        let mut cmd = std::process::Command::new(&bv);
        cmd.arg("add");
        for tool in &fixture.tools {
            cmd.arg(format!("{}@{}", tool.id, tool.version));
        }
        if !registry.is_empty() {
            cmd.arg("--registry").arg(&registry);
        }
        cmd.current_dir(work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = cmd.status()?;
        if !status.success() {
            bail!("bv add failed");
        }

        // Evict images so the sync below measures a true cold download.
        evict_docker_images(work_dir);

        // Cold parallel pull. This is the dominant install cost.
        let status = std::process::Command::new(&bv)
            .arg("sync")
            .current_dir(work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("bv sync failed");
        }

        let footprint = lockfile_image_size(work_dir);
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let bv = find_bv()?;
        let tool = fixture.tools.first().expect("non-empty fixture");
        let start = Instant::now();
        let status = std::process::Command::new(&bv)
            .args(["run", &tool.id, "--", "--version"])
            .current_dir(work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} failed", tool.id);
        }
        Ok(elapsed)
    }
}

/// Sum `image_size_bytes` for all tools in the bv.lock written by `bv add`.
fn lockfile_image_size(work_dir: &std::path::Path) -> u64 {
    let lock_path = work_dir.join("bv.lock");
    let contents = match std::fs::read_to_string(&lock_path) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let doc: toml::Value = match toml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let tools = match doc.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return 0,
    };
    tools
        .values()
        .filter_map(|entry| {
            entry
                .get("image_size_bytes")
                .and_then(|v| v.as_integer())
                .map(|n| n as u64)
        })
        .sum()
}

/// Remove Docker images for all tools recorded in `bv.lock` so the next
/// `bv run` triggers a cold download from the registry.
fn evict_docker_images(work_dir: &std::path::Path) {
    let lock_path = work_dir.join("bv.lock");
    let contents = match std::fs::read_to_string(&lock_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let doc: toml::Value = match toml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return,
    };
    let tools = match doc.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return,
    };
    for entry in tools.values() {
        let reference = entry.get("image_reference").and_then(|v| v.as_str());
        let digest = entry.get("image_digest").and_then(|v| v.as_str());
        if let (Some(r), Some(d)) = (reference, digest) {
            let full_ref = format!("{r}@{d}");
            let _ = std::process::Command::new("docker")
                .args(["rmi", "-f", &full_ref])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

struct ApptainerPath;

impl InstallPath for ApptainerPath {
    fn name(&self) -> &str {
        "apptainer"
    }

    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)> {
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
        let start = Instant::now();
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

/// A conda-compatible CLI (mamba, micromamba, conda).
struct CondaLikePath {
    name: String,
    bin: PathBuf,
}

impl InstallPath for CondaLikePath {
    fn name(&self) -> &str {
        &self.name
    }

    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)> {
        let env_dir = work_dir.join("env");
        // Isolated package cache: forces a cold download instead of hitting
        // the global ~/miniforge3/pkgs/ cache.
        let pkgs_dir = work_dir.join("pkgs");

        if env_dir.exists() {
            std::fs::remove_dir_all(&env_dir).ok();
        }
        if pkgs_dir.exists() {
            std::fs::remove_dir_all(&pkgs_dir).ok();
        }
        std::fs::create_dir_all(&pkgs_dir)?;

        let mut args = vec![
            "create".to_string(),
            "-p".to_string(), env_dir.to_str().unwrap().to_string(),
            "-c".to_string(), "bioconda".to_string(),
            "-c".to_string(), "conda-forge".to_string(),
        ];
        for tool in &fixture.tools {
            args.push(format!("{}={}", tool.id, to_conda_version(&tool.version)));
        }
        args.push("--yes".to_string());

        // CONDA_PKGS_DIRS isolates the package cache to work_dir so each
        // fixture run downloads fresh rather than hitting ~/miniforge3/pkgs/.
        let status = std::process::Command::new(&self.bin)
            .args(&args)
            .env("CONDA_PKGS_DIRS", &pkgs_dir)
            .current_dir(work_dir)
            .status()?;
        if !status.success() {
            bail!("{} create failed for fixture '{}'", self.name, fixture.name);
        }

        let footprint = dir_size(&env_dir).unwrap_or(0);
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let tool = fixture.tools.first().expect("non-empty fixture");
        let env_dir = work_dir.join("env");
        let start = Instant::now();
        let status = std::process::Command::new(&self.bin)
            .args(["run", "-p", env_dir.to_str().unwrap(), &tool.id, "--version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} ({}) failed", tool.id, self.name);
        }
        Ok(elapsed)
    }
}

/// micromamba 2.x — same install interface as mamba but `run` uses different flags.
struct MicromambaPath {
    bin: PathBuf,
}

impl InstallPath for MicromambaPath {
    fn name(&self) -> &str {
        "micromamba"
    }

    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)> {
        let env_dir = work_dir.join("env");
        let pkgs_dir = work_dir.join("pkgs");

        if env_dir.exists() {
            std::fs::remove_dir_all(&env_dir).ok();
        }
        if pkgs_dir.exists() {
            std::fs::remove_dir_all(&pkgs_dir).ok();
        }
        std::fs::create_dir_all(&pkgs_dir)?;

        let mut args = vec![
            "create".to_string(),
            "-p".to_string(), env_dir.to_str().unwrap().to_string(),
            "-c".to_string(), "bioconda".to_string(),
            "-c".to_string(), "conda-forge".to_string(),
        ];
        for tool in &fixture.tools {
            args.push(format!("{}={}", tool.id, to_conda_version(&tool.version)));
        }
        args.push("--yes".to_string());

        let status = std::process::Command::new(&self.bin)
            .args(&args)
            .env("CONDA_PKGS_DIRS", &pkgs_dir)
            // micromamba needs a writable root prefix; point it at work_dir.
            .env("MAMBA_ROOT_PREFIX", work_dir)
            .current_dir(work_dir)
            .status()?;
        if !status.success() {
            bail!("micromamba create failed for fixture '{}'", fixture.name);
        }

        let footprint = dir_size(&env_dir).unwrap_or(0);
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let tool = fixture.tools.first().expect("non-empty fixture");
        let env_dir = work_dir.join("env");
        let start = Instant::now();
        let status = std::process::Command::new(&self.bin)
            .args(["run", "-p", env_dir.to_str().unwrap(), &tool.id, "--version"])
            .env("MAMBA_ROOT_PREFIX", work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(work_dir)
            .status()?;
        let elapsed = start.elapsed();
        if !status.success() {
            bail!("cold-run of {} (micromamba) failed", tool.id);
        }
        Ok(elapsed)
    }
}

struct PixiPath;

impl InstallPath for PixiPath {
    fn name(&self) -> &str {
        "pixi"
    }

    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)> {
        let pixi_dir = work_dir.join(".pixi");
        let pixi_toml = work_dir.join("pixi.toml");
        let pixi_lock = work_dir.join("pixi.lock");
        // Isolated rattler cache: prevents hits on the global ~/.rattler cache.
        let cache_dir = work_dir.join("rattler-cache");

        if pixi_dir.exists() {
            std::fs::remove_dir_all(&pixi_dir).ok();
        }
        if pixi_toml.exists() {
            std::fs::remove_file(&pixi_toml).ok();
        }
        if pixi_lock.exists() {
            std::fs::remove_file(&pixi_lock).ok();
        }
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir).ok();
        }

        let pixi = |args: &[&str]| -> std::process::Command {
            let mut cmd = std::process::Command::new("pixi");
            cmd.args(args)
                .env("RATTLER_CACHE_DIR", &cache_dir)
                .current_dir(work_dir);
            cmd
        };

        // Init with conda-forge (default), then add bioconda.
        if !pixi(&["init", "."]).status()?.success() {
            bail!("pixi init failed");
        }
        if !pixi(&["project", "channel", "add", "bioconda"]).status()?.success() {
            bail!("pixi project channel add bioconda failed");
        }

        for tool in &fixture.tools {
            let spec = format!("{}={}", tool.id, to_conda_version(&tool.version));
            if !pixi(&["add", &spec]).status()?.success() {
                bail!("pixi add {} failed", tool.id);
            }
        }

        let footprint = dir_size(&pixi_dir).unwrap_or(0);
        Ok((footprint, Duration::ZERO))
    }

    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration> {
        let tool = fixture.tools.first().expect("non-empty fixture");
        let bin = work_dir
            .join(".pixi")
            .join("envs")
            .join("default")
            .join("bin")
            .join(&tool.id);
        let start = Instant::now();
        let status = std::process::Command::new(&bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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

/// Locate a conda-compatible binary (mamba, conda, micromamba) by searching
/// well-known paths.
fn find_conda_like(name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let candidates: Vec<PathBuf> = vec![
        PathBuf::from(name),
        home.join(format!("miniforge3/bin/{name}")),
        home.join(format!("mambaforge/bin/{name}")),
        home.join(format!("miniconda3/bin/{name}")),
        home.join(format!("anaconda3/bin/{name}")),
        PathBuf::from(format!("/opt/homebrew/bin/{name}")),
        PathBuf::from(format!("/usr/local/bin/{name}")),
        PathBuf::from(format!("/opt/conda/bin/{name}")),
    ];
    for path in &candidates {
        if path.exists() || which::which(path).is_ok() {
            return Ok(path.clone());
        }
    }
    bail!("{name} binary not found; tried common paths")
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

fn to_conda_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    let mut end = parts.len();
    while end > 1 && parts[end - 1] == "0" {
        end -= 1;
    }
    parts[..end].join(".")
}
