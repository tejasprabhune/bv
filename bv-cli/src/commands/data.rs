use std::io::Write as _;
use std::path::Path;

use anyhow::Context;
use futures_util::StreamExt as _;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Stream};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;

use bv_core::cache::CacheLayout;
use bv_core::data::PostDownloadAction;
use bv_index::{GitIndex, IndexBackend as _};

use crate::commands::add::format_size;

pub async fn fetch(
    datasets: &[String],
    registry_flag: Option<&str>,
    yes: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_toml_path = cwd.join("bv.toml");

    let bv_toml = bv_core::project::BvToml::from_path(&bv_toml_path).ok();
    let registry_url = crate::registry::resolve_registry_url(registry_flag, bv_toml.as_ref());

    let cache = CacheLayout::new();
    let index = crate::registry::open_index(&registry_url, &cache);

    let refreshed = index
        .refresh_if_stale(crate::registry::STALE_TTL)
        .with_context(|| format!("registry refresh failed for '{}'", registry_url))?;
    crate::registry::maybe_print_refresh(refreshed);

    for spec in datasets {
        let (id, version) = parse_dataset_spec(spec);
        fetch_one(&id, version.as_deref(), &index, &cache, yes).await?;
    }

    Ok(())
}

fn parse_dataset_spec(spec: &str) -> (String, Option<String>) {
    if let Some((id, ver)) = spec.split_once('@') {
        (id.to_string(), Some(ver.to_string()))
    } else {
        (spec.to_string(), None)
    }
}

async fn fetch_one(
    id: &str,
    version: Option<&str>,
    index: &GitIndex,
    cache: &CacheLayout,
    yes: bool,
) -> anyhow::Result<()> {
    let manifest = index
        .get_data_manifest(id, version)
        .with_context(|| format!("could not resolve dataset '{id}' in registry"))?;

    let ver = &manifest.data.version;
    let final_dir = cache.data_dir(id, ver);

    if final_dir.exists() {
        eprintln!(
            "  {} {id}@{ver} already in cache",
            "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        return Ok(());
    }

    // Size confirmation
    if !yes {
        let size_str = match manifest.data.size_bytes {
            Some(b) => format_size(b),
            None => "unknown size".to_string(),
        };
        eprint!("  {id}@{ver} is {size_str}. Continue? [y/N] ");
        std::io::stderr().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let answer = line.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            eprintln!(
                "  {}",
                "Aborted.".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
            );
            return Ok(());
        }
    }

    if manifest.data.source_urls.is_empty() {
        anyhow::bail!("dataset '{id}' has no source_urls in its manifest");
    }

    // Atomic-fetch contract: download into a per-fetch staging directory under
    // tmp_dir(), then rename it into place as the very last step. This makes
    // the cache-hit guard above (`final_dir.exists()`) correct: `final_dir`
    // exists iff a previous fetch completed cleanly.
    let tmp_dir = cache.tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    let staging_dir = tmp_dir.join(format!("staging-{id}-{ver}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging_dir);
    std::fs::create_dir_all(&staging_dir)?;

    // RAII guard: if we exit this scope without committing, blow away the
    // staging dir so a partial fetch can't leak.
    struct StagingGuard<'a> {
        path: &'a std::path::Path,
        committed: bool,
    }
    impl Drop for StagingGuard<'_> {
        fn drop(&mut self) {
            if !self.committed {
                let _ = std::fs::remove_dir_all(self.path);
            }
        }
    }
    let mut guard = StagingGuard {
        path: &staging_dir,
        committed: false,
    };

    eprintln!(
        "  {} {id}@{ver}",
        "Fetching".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string())
    );

    let mut downloaded: Vec<std::path::PathBuf> = Vec::new();
    for (i, url) in manifest.data.source_urls.iter().enumerate() {
        let filename = url
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("download");
        let tmp_path = staging_dir.join(format!("{id}-{ver}-{filename}"));

        // The primary file's sha256 is enforced when declared; sidecars
        // (e.g. a `.tbi` alongside a `.vcf.gz`) are never integrity-checked.
        let expected_sha = if i == 0 {
            manifest.data.sha256.as_deref()
        } else {
            None
        };
        download_verified(url, &tmp_path, expected_sha, manifest.data.size_bytes).await?;
        downloaded.push(tmp_path);
    }

    // Build the staged final layout under staging_dir/final, then promote it
    // to the real final_dir with a single atomic rename.
    let staged_final = staging_dir.join("final");
    std::fs::create_dir_all(&staged_final)?;

    // Apply the post-download action to the primary file only; sidecars are
    // moved into the staged final directory as-is.
    let primary = downloaded.remove(0);
    match manifest.data.post_download_action {
        PostDownloadAction::Noop => {
            let dest = staged_final.join(primary.file_name().unwrap());
            std::fs::rename(&primary, &dest).context("failed to stage downloaded file")?;
        }
        PostDownloadAction::Extract => {
            extract_archive(&primary, &staged_final)?;
            let _ = std::fs::remove_file(&primary);
        }
        PostDownloadAction::Decompress => {
            decompress_gzip(&primary, &staged_final)?;
            let _ = std::fs::remove_file(&primary);
        }
    }
    for extra in downloaded {
        let dest = staged_final.join(extra.file_name().unwrap());
        std::fs::rename(&extra, &dest).context("failed to stage downloaded sidecar file")?;
    }

    // Promote the staged dir to the cache. final_dir's parent must exist; the
    // cache layout owns its parent so create it just in case.
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&staged_final, &final_dir)
        .context("failed to move staged dataset to cache")?;
    guard.committed = true;

    eprintln!(
        "  {} {id}@{ver}  {}",
        "Fetched".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        final_dir
            .display()
            .to_string()
            .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
    );

    Ok(())
}

async fn download_verified(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    size_hint: Option<u64>,
) -> anyhow::Result<()> {
    // NOTE: resume support was removed. The previous implementation reset the
    // hasher on resume so the sha256 check covered only the newly downloaded
    // bytes, not the full file — silently letting corrupted partial files
    // through. A correct resume design needs to either rehash the existing
    // bytes before continuing, or track per-chunk hashes. Until that exists,
    // every fetch is a fresh download.
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .context("HTTP request failed")?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", response.status());
    }

    let total = response.content_length().or(size_hint).unwrap_or(0);
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template("  {bar:40.cyan/blue} {bytes}/{total_bytes}  {eta}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .await
        .context("failed to open destination file")?;

    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("download stream error")?;
        hasher.update(&bytes);
        file.write_all(&bytes).await.context("write failed")?;
        bar.inc(bytes.len() as u64);
    }
    file.flush().await?;
    bar.finish_and_clear();

    if let Some(expected) = expected_sha256 {
        let digest_bytes = hasher.finalize();
        let hex: String = digest_bytes.iter().map(|b| format!("{b:02x}")).collect();
        let actual = format!("sha256:{hex}");
        if actual != expected {
            let _ = std::fs::remove_file(dest);
            anyhow::bail!(
                "SHA-256 mismatch for {url}\n  expected {expected}\n  got      {actual}\n\
                 The downloaded file has been deleted."
            );
        }
    }

    Ok(())
}

fn decompress_gzip(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let stem = archive
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".gz"))
        .unwrap_or_else(|| {
            archive
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("decompressed")
        });
    let out_path = dest.join(stem);
    let f_in = std::fs::File::open(archive)
        .with_context(|| format!("failed to open {} for decompression", archive.display()))?;
    // MultiGzDecoder handles concatenated gzip members (bgzip-style files like
    // tabix-indexed VCFs); plain single-member gzip works through it too.
    let mut decoder = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(f_in));
    let f_out = std::fs::File::create(&out_path)
        .with_context(|| format!("failed to create {}", out_path.display()))?;
    let mut writer = std::io::BufWriter::new(f_out);
    std::io::copy(&mut decoder, &mut writer).context("gzip decompression failed")?;
    Ok(())
}

fn extract_archive(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .context("failed to launch tar")?;

    if !status.success() {
        anyhow::bail!("tar extraction failed for {}", archive.display());
    }
    Ok(())
}

pub fn list() -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let data_root = cache.root().join("data");

    if !data_root.exists() {
        eprintln!(
            "  {}",
            "No reference datasets in cache. Use `bv data fetch <dataset>` to download one."
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        return Ok(());
    }

    let mut rows: Vec<(String, String, u64)> = Vec::new();
    for id_entry in std::fs::read_dir(&data_root)?.flatten() {
        if !id_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = id_entry.file_name().to_string_lossy().to_string();
        for ver_entry in std::fs::read_dir(id_entry.path())?.flatten() {
            if !ver_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let ver = ver_entry.file_name().to_string_lossy().to_string();
            let size = dir_size_bytes(&ver_entry.path());
            rows.push((id.clone(), ver, size));
        }
    }

    if rows.is_empty() {
        eprintln!(
            "  {}",
            "No reference datasets in cache."
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        return Ok(());
    }

    rows.sort();
    eprintln!(
        "  {:<22} {:<15} {}",
        "dataset".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        "version".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        "size".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
    );
    for (id, ver, size) in &rows {
        eprintln!("  {:<22} {:<15} {}", id, ver, format_size(*size));
    }

    Ok(())
}

/// HEAD-check every dataset in the registry. Compares the manifest's
/// declared `size_bytes` (and primary URL reachability) to what the server
/// reports right now. Fast: no downloads.
pub async fn verify(
    registry_flag: Option<&str>,
    filter: Option<&str>,
    jobs: usize,
    size_tolerance: f64,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    use futures_util::stream::{FuturesUnordered, StreamExt};
    use tokio::sync::Semaphore;

    let cwd = std::env::current_dir()?;
    let bv_toml = bv_core::project::BvToml::from_path(&cwd.join("bv.toml")).ok();
    let registry_url = crate::registry::resolve_registry_url(registry_flag, bv_toml.as_ref());
    let cache = CacheLayout::new();
    let index = crate::registry::open_index(&registry_url, &cache);

    index.refresh().context("registry refresh failed")?;

    let mut datasets = index.list_datasets()?;
    if let Some(f) = filter {
        datasets.retain(|d| d.contains(f));
    }
    let jobs = jobs.max(1);

    eprintln!(
        "  {} {} dataset(s) from {}  (jobs: {})\n",
        "Data verify:".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        datasets.len(),
        registry_url,
        jobs,
    );

    // Collect (id, version, urls, declared_size, declared_sha) up front. The
    // index access is sync; doing it here keeps the concurrent loop pure
    // network I/O.
    struct Job {
        id: String,
        version: String,
        primary_url: String,
        declared_size: Option<u64>,
        declared_sha: Option<String>,
    }
    let mut jobs_vec: Vec<Job> = Vec::with_capacity(datasets.len());
    for id in &datasets {
        match index.get_data_manifest(id, None) {
            Ok(m) => {
                let primary = m.data.source_urls.first().cloned().unwrap_or_default();
                jobs_vec.push(Job {
                    id: id.clone(),
                    version: m.data.version,
                    primary_url: primary,
                    declared_size: m.data.size_bytes,
                    declared_sha: m.data.sha256,
                });
            }
            Err(e) => {
                eprintln!("  ERR   {id}: manifest load: {e}");
            }
        }
    }

    enum Verdict {
        Ok { actual_size: Option<u64> },
        Mismatch { declared: u64, actual: u64 },
        BrokenUrl { status: String },
        NoUrl,
    }

    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build http client")?,
    );
    let sem = Arc::new(Semaphore::new(jobs));
    let mut tasks: FuturesUnordered<tokio::task::JoinHandle<(Job, Verdict)>> =
        FuturesUnordered::new();

    for job in jobs_vec {
        let sem = sem.clone();
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return (
                        job,
                        Verdict::BrokenUrl {
                            status: "semaphore closed".into(),
                        },
                    );
                }
            };
            if job.primary_url.is_empty() {
                return (job, Verdict::NoUrl);
            }
            // Many FTP/CDN servers reject HEAD; fall back to a Range:0-0 GET
            // which costs the same one round trip and always returns headers.
            let resp = client
                .get(&job.primary_url)
                .header("Range", "bytes=0-0")
                .send()
                .await;
            let verdict = match resp {
                Ok(r) if r.status().is_success() => {
                    let actual = parse_total_size(&r);
                    if let (Some(declared), Some(actual)) = (job.declared_size, actual) {
                        let ratio = (declared as f64 - actual as f64).abs() / actual.max(1) as f64;
                        if ratio > size_tolerance {
                            Verdict::Mismatch { declared, actual }
                        } else {
                            Verdict::Ok {
                                actual_size: Some(actual),
                            }
                        }
                    } else {
                        Verdict::Ok {
                            actual_size: actual,
                        }
                    }
                }
                Ok(r) => Verdict::BrokenUrl {
                    status: format!("HTTP {}", r.status()),
                },
                Err(e) => Verdict::BrokenUrl {
                    status: e.to_string(),
                },
            };
            (job, verdict)
        }));
    }

    let mut results: Vec<(Job, Verdict)> = Vec::new();
    while let Some(joined) = tasks.next().await {
        match joined {
            Ok(t) => results.push(t),
            Err(e) => eprintln!("  task join error: {e}"),
        }
    }
    results.sort_by(|a, b| a.0.id.cmp(&b.0.id));

    let mut ok = 0;
    let mut mismatch = 0;
    let mut broken = 0;
    let mut no_url = 0;

    eprintln!(
        "  {}",
        "── Summary ──".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    let id_w = results.iter().map(|(j, _)| j.id.len()).max().unwrap_or(7);
    let ver_w = results
        .iter()
        .map(|(j, _)| j.version.len())
        .max()
        .unwrap_or(7);

    for (job, verdict) in &results {
        let (label, detail) = match verdict {
            Verdict::Ok { actual_size } => {
                ok += 1;
                let label = "OK  "
                    .if_supports_color(Stream::Stderr, |t| t.green().bold().to_string())
                    .to_string();
                let actual = actual_size
                    .map(format_size)
                    .unwrap_or_else(|| "(no size header)".to_string());
                let sha = if job.declared_sha.is_some() {
                    " [sha256 declared, not verified]"
                } else {
                    ""
                };
                (label, format!("actual={actual}{sha}"))
            }
            Verdict::Mismatch { declared, actual } => {
                mismatch += 1;
                let label = "DIFF"
                    .if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
                    .to_string();
                (
                    label,
                    format!(
                        "declared={}  actual={}",
                        format_size(*declared),
                        format_size(*actual)
                    ),
                )
            }
            Verdict::BrokenUrl { status } => {
                broken += 1;
                let label = "FAIL"
                    .if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
                    .to_string();
                (label, status.clone())
            }
            Verdict::NoUrl => {
                no_url += 1;
                let label = "FAIL"
                    .if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
                    .to_string();
                (label, "no source_urls".into())
            }
        };
        eprintln!(
            "  {}  {:<id_w$}  {:<ver_w$}  {}",
            label,
            job.id,
            job.version,
            detail,
            id_w = id_w,
            ver_w = ver_w,
        );
    }
    eprintln!(
        "\n  {} ok: {ok}  diff: {mismatch}  fail: {}  ({} dataset(s))",
        "Totals:".if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
        broken + no_url,
        results.len(),
    );

    if broken > 0 || no_url > 0 {
        anyhow::bail!("one or more datasets have unreachable URLs");
    }
    Ok(())
}

/// Extract total file size from a Range:0-0 response. Servers that honor
/// Range return 206 with Content-Range like `bytes 0-0/123456`; servers that
/// ignore Range return 200 with regular Content-Length.
fn parse_total_size(resp: &reqwest::Response) -> Option<u64> {
    if let Some(cr) = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        && let Some(total) = cr.rsplit('/').next()
        && let Ok(n) = total.parse::<u64>()
    {
        return Some(n);
    }
    resp.content_length()
}

fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                dir_size_bytes(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}
