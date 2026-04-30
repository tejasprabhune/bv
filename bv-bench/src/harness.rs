use std::time::{Duration, Instant};

use anyhow::Result;

use crate::fixture::Fixture;
use crate::report::BenchResult;

/// Backend flags controlling which install path to exercise.
#[derive(Debug, Clone, Default)]
pub struct BenchFlags {
    /// Require Linux Docker (skip on macOS).
    pub linux_only: bool,
    /// Use Apptainer instead of Docker.
    pub apptainer: bool,
}

/// Trait that each install path implements.
/// Adding a new path (e.g. factored OCI) is one `impl`.
pub trait InstallPath: Send + Sync {
    fn name(&self) -> &str;

    /// Install all tools in the fixture into `work_dir`, returning the
    /// disk footprint in bytes and the install duration.
    fn install(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<(u64, Duration)>;

    /// Run a single cold-cache invocation of the first tool in the fixture
    /// (e.g. `samtools --version`) and return the wall-clock time.
    fn cold_run(&self, fixture: &Fixture, work_dir: &std::path::Path) -> Result<Duration>;
}

/// Run the full benchmark suite against `path` for all `fixtures`.
pub fn run_suite(
    path: &dyn InstallPath,
    fixtures: &[Fixture],
    flags: &BenchFlags,
    work_root: &std::path::Path,
) -> Vec<BenchResult> {
    if flags.linux_only && !cfg!(target_os = "linux") {
        eprintln!("--linux-only: skipping on non-Linux host");
        return vec![];
    }

    fixtures
        .iter()
        .map(|fixture| {
            let work_dir = work_root.join(&fixture.name);
            std::fs::create_dir_all(&work_dir).expect("create work_dir");

            let install_start = Instant::now();
            let install_result = path.install(fixture, &work_dir);
            let install_duration = install_start.elapsed();

            let (footprint_bytes, install_error) = match install_result {
                Ok((bytes, _)) => (bytes, None),
                Err(e) => {
                    eprintln!(
                        "  install failed [{}/{}]: {}",
                        path.name(),
                        fixture.name,
                        e
                    );
                    (0, Some(e.to_string()))
                }
            };

            let (cold_run_duration, cold_run_error) = if install_error.is_none() {
                match path.cold_run(fixture, &work_dir) {
                    Ok(d) => (d, None),
                    Err(e) => {
                        eprintln!(
                            "  cold-run failed [{}/{}]: {}",
                            path.name(),
                            fixture.name,
                            e
                        );
                        (Duration::ZERO, Some(e.to_string()))
                    }
                }
            } else {
                (Duration::ZERO, None)
            };

            let error = install_error.or(cold_run_error);

            BenchResult {
                fixture_name: fixture.name.clone(),
                tool_count: fixture.tools.len(),
                path_name: path.name().to_string(),
                install_duration,
                footprint_bytes,
                cold_run_duration,
                error,
                timestamp: chrono::Utc::now(),
            }
        })
        .collect()
}
