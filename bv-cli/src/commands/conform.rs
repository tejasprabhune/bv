use std::sync::Arc;

use anyhow::Context;
use futures_util::stream::{FuturesUnordered, StreamExt};
use owo_colors::{OwoColorize, Stream};
use semver::VersionReq;
use tokio::sync::Semaphore;

use bv_core::cache::CacheLayout;
use bv_core::manifest::Manifest;
use bv_index::IndexBackend as _;
use bv_runtime::ContainerRuntime as _;

use crate::registry::{STALE_TTL, maybe_print_refresh, open_index, resolve_registry_url};

enum Outcome {
    Pass(std::time::Duration),
    Fail {
        duration: std::time::Duration,
        messages: Vec<String>,
    },
    /// Setup error (image pull, runtime issue) — distinguished from a probe
    /// failure because it does not indicate the tool itself is broken.
    Error(String),
    Skipped(&'static str),
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Pass(_) => "PASS",
            Outcome::Fail { .. } => "FAIL",
            Outcome::Error(_) => "ERR ",
            Outcome::Skipped(_) => "SKIP",
        }
    }

    fn colored(&self) -> String {
        let s = self.label();
        match self {
            Outcome::Pass(_) => s
                .if_supports_color(Stream::Stderr, |t| t.green().bold().to_string())
                .to_string(),
            Outcome::Fail { .. } | Outcome::Error(_) => s
                .if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
                .to_string(),
            Outcome::Skipped(_) => s
                .if_supports_color(Stream::Stderr, |t| t.yellow().to_string())
                .to_string(),
        }
    }
}

pub async fn run(
    tool: &str,
    registry_flag: Option<&str>,
    backend_flag: Option<&str>,
) -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let registry_url = resolve_registry_url(registry_flag, None);
    let index = open_index(&registry_url, &cache);

    let refreshed = index
        .refresh_if_stale(STALE_TTL)
        .context("registry refresh failed")?;
    maybe_print_refresh(refreshed);

    let manifest = index
        .get_manifest(tool, &VersionReq::STAR)
        .with_context(|| format!("tool '{}' not found in registry", tool))?;

    let runtime = crate::runtime_select::resolve_runtime(backend_flag, None)?;
    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    eprintln!(
        "  {} {}@{}",
        "Running conformance for"
            .if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        tool,
        manifest.tool.version
    );

    let outcome = run_one(&manifest, &runtime, /*verbose=*/ true);

    match &outcome {
        Outcome::Pass(d) => eprintln!(
            "\n  {} {} in {:.1}s",
            outcome.colored(),
            tool,
            d.as_secs_f32()
        ),
        Outcome::Fail { duration, .. } => {
            eprintln!(
                "\n  {} {} in {:.1}s",
                outcome.colored(),
                tool,
                duration.as_secs_f32()
            );
            anyhow::bail!("conformance failed for '{}'", tool);
        }
        Outcome::Error(msg) => {
            anyhow::bail!("conformance error for '{}': {msg}", tool);
        }
        Outcome::Skipped(reason) => {
            eprintln!("  {} skipped: {reason}", outcome.colored());
        }
    }
    Ok(())
}

pub async fn run_all(
    registry_flag: Option<&str>,
    backend_flag: Option<&str>,
    filter: Option<&str>,
    skip_gpu: bool,
    skip_reference_data: bool,
    skip_deprecated: bool,
    jobs: usize,
) -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let registry_url = resolve_registry_url(registry_flag, None);
    let index = open_index(&registry_url, &cache);

    let refreshed = index
        .refresh_if_stale(STALE_TTL)
        .context("registry refresh failed")?;
    maybe_print_refresh(refreshed);

    let runtime = crate::runtime_select::resolve_runtime(backend_flag, None)?;
    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    let mut tools = index.list_tools().context("failed to list tools")?;
    tools.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(f) = filter {
        tools.retain(|t| t.id.contains(f));
    }

    let jobs = jobs.max(1);
    eprintln!(
        "  {} {} tool(s) from {}  (jobs: {})\n",
        "Conformance walk:".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        tools.len(),
        registry_url,
        jobs,
    );

    // Pre-load manifests sequentially. Index access is synchronous and
    // shared; doing it under the semaphore would needlessly serialize.
    // Keeps a stable per-tool record even when manifest load itself fails.
    let mut prepared: Vec<(String, String, Result<Manifest, String>)> =
        Vec::with_capacity(tools.len());
    for summary in &tools {
        match index.get_manifest(&summary.id, &VersionReq::STAR) {
            Ok(m) => {
                let version = m.tool.version.clone();
                prepared.push((summary.id.clone(), version, Ok(m)));
            }
            Err(e) => {
                prepared.push((summary.id.clone(), "?".into(), Err(e.to_string())));
            }
        }
    }

    let started = std::time::Instant::now();
    let sem = Arc::new(Semaphore::new(jobs));
    let mut tasks: FuturesUnordered<tokio::task::JoinHandle<(String, String, Outcome)>> =
        FuturesUnordered::new();

    for (id, version, manifest_res) in prepared {
        let sem = sem.clone();
        let runtime = runtime.clone();
        tasks.push(tokio::spawn(async move {
            let permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return (id, version, Outcome::Error("semaphore closed".into())),
            };
            let manifest = match manifest_res {
                Ok(m) => m,
                Err(e) => {
                    return (id, version, Outcome::Error(format!("manifest load: {e}")));
                }
            };
            let outcome = if skip_deprecated && manifest.tool.deprecated {
                Outcome::Skipped("deprecated")
            } else if skip_gpu && requires_gpu(&manifest) {
                Outcome::Skipped("requires GPU")
            } else if skip_reference_data && requires_reference_data(&manifest) {
                Outcome::Skipped("requires reference data")
            } else {
                eprintln!(
                    "  {} {}@{}",
                    "==>".if_supports_color(Stream::Stderr, |t| t.cyan().to_string()),
                    id,
                    version
                );
                let result = tokio::task::spawn_blocking(move || {
                    run_one(&manifest, &runtime, /*verbose=*/ false)
                })
                .await;
                drop(permit);
                match result {
                    Ok(o) => o,
                    Err(e) => Outcome::Error(format!("task panic for {id}@{version}: {e}")),
                }
            };
            (id, version, outcome)
        }));
    }

    let mut results: Vec<(String, String, Outcome)> = Vec::with_capacity(tasks.len());
    while let Some(joined) = tasks.next().await {
        let (id, version, outcome) = match joined {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  task join error: {e}");
                continue;
            }
        };
        eprintln!("  {}  {}@{}", outcome.colored(), id, version);
        if let Outcome::Fail { messages, .. } = &outcome {
            for m in messages {
                eprintln!("      {m}");
            }
        }
        if let Outcome::Error(msg) = &outcome {
            eprintln!("      {msg}");
        }
        results.push((id, version, outcome));
    }

    // Restore alphabetical order for the summary table.
    results.sort_by(|a, b| a.0.cmp(&b.0));
    print_summary(&results, started.elapsed());

    let any_fail = results
        .iter()
        .any(|(_, _, o)| matches!(o, Outcome::Fail { .. } | Outcome::Error(_)));
    if any_fail {
        anyhow::bail!("one or more tools failed conformance");
    }
    Ok(())
}

fn run_one(
    manifest: &Manifest,
    runtime: &dyn bv_runtime::ContainerRuntime,
    verbose: bool,
) -> Outcome {
    let started = std::time::Instant::now();
    let image_digest = match bv_conformance::verify_image_reachable(manifest, runtime) {
        Ok(d) => d,
        Err(e) => return Outcome::Error(format!("image pull: {e}")),
    };
    if verbose {
        eprintln!(
            "    image pulled ({})",
            &image_digest.0[..image_digest.0.len().min(20)]
        );
    }

    let result = match bv_conformance::run(manifest, &image_digest.0, runtime) {
        Ok(r) => r,
        Err(e) => return Outcome::Error(format!("conformance run: {e}")),
    };

    if verbose {
        for msg in &result.messages {
            if result.passed {
                eprintln!("    {msg}");
            } else {
                eprintln!(
                    "    {} {msg}",
                    "fail".if_supports_color(Stream::Stderr, |t| t.red().to_string())
                );
            }
        }
    }

    if result.passed {
        Outcome::Pass(started.elapsed())
    } else {
        Outcome::Fail {
            duration: started.elapsed(),
            messages: result.messages,
        }
    }
}

fn requires_gpu(manifest: &Manifest) -> bool {
    manifest
        .tool
        .hardware
        .gpu
        .as_ref()
        .is_some_and(|g| g.required)
}

fn requires_reference_data(manifest: &Manifest) -> bool {
    manifest.tool.reference_data.values().any(|d| d.required)
}

fn print_summary(results: &[(String, String, Outcome)], total: std::time::Duration) {
    let mut pass = 0;
    let mut fail = 0;
    let mut err = 0;
    let mut skip = 0;
    for (_, _, o) in results {
        match o {
            Outcome::Pass(_) => pass += 1,
            Outcome::Fail { .. } => fail += 1,
            Outcome::Error(_) => err += 1,
            Outcome::Skipped(_) => skip += 1,
        }
    }
    let max_id = results.iter().map(|(id, _, _)| id.len()).max().unwrap_or(4);
    let max_ver = results.iter().map(|(_, v, _)| v.len()).max().unwrap_or(7);

    eprintln!(
        "\n  {}",
        "── Summary ──".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    for (id, version, outcome) in results {
        let detail = match outcome {
            Outcome::Pass(d) => format!("{:.1}s", d.as_secs_f32()),
            Outcome::Fail { duration, .. } => format!("{:.1}s", duration.as_secs_f32()),
            Outcome::Error(msg) => truncate(msg, 60),
            Outcome::Skipped(reason) => (*reason).into(),
        };
        eprintln!(
            "  {}  {:<id_w$}  {:<ver_w$}  {}",
            outcome.colored(),
            id,
            version,
            detail,
            id_w = max_id,
            ver_w = max_ver,
        );
    }
    eprintln!(
        "\n  {} pass: {pass}  fail: {fail}  err: {err}  skip: {skip}  ({:.1}s total)",
        "Totals:".if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
        total.as_secs_f32(),
    );
}

fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ").trim().to_string();
    if one_line.len() <= n {
        one_line
    } else {
        format!("{}…", &one_line[..n.saturating_sub(1)])
    }
}
