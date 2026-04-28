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
        let size_str = format_size(manifest.data.size_bytes);
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

    let tmp_dir = cache.tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    std::fs::create_dir_all(&final_dir)?;

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
        let tmp_path = tmp_dir.join(format!("{id}-{ver}-{filename}"));

        // The primary file's sha256 is enforced; sidecars (e.g. a `.tbi`
        // alongside a `.vcf.gz`) are downloaded without integrity checking.
        let expected_sha = if i == 0 {
            Some(manifest.data.sha256.as_str())
        } else {
            None
        };
        download_verified(url, &tmp_path, expected_sha, manifest.data.size_bytes).await?;
        downloaded.push(tmp_path);
    }

    // Apply the post-download action to the primary file only; sidecars are
    // moved into the final cache directory as-is.
    let primary = downloaded.remove(0);
    match manifest.data.post_download_action {
        PostDownloadAction::Noop => {
            let dest = final_dir.join(primary.file_name().unwrap());
            std::fs::rename(&primary, &dest).context("failed to move downloaded file to cache")?;
        }
        PostDownloadAction::Extract => {
            extract_archive(&primary, &final_dir)?;
            let _ = std::fs::remove_file(&primary);
        }
        PostDownloadAction::Decompress => {
            decompress_gzip(&primary, &final_dir)?;
            let _ = std::fs::remove_file(&primary);
        }
    }
    for extra in downloaded {
        let dest = final_dir.join(extra.file_name().unwrap());
        std::fs::rename(&extra, &dest)
            .context("failed to move downloaded sidecar file to cache")?;
    }

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
    size_hint: u64,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Support resume: check if partial file exists.
    let existing_bytes = dest.metadata().map(|m| m.len()).unwrap_or(0);

    let response = if existing_bytes > 0 {
        let req = client
            .get(url)
            .header("Range", format!("bytes={existing_bytes}-"))
            .send()
            .await
            .context("HTTP request failed")?;
        // Server may not honor Range; if it returns 200 restart from scratch.
        if req.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            req
        } else {
            // Restart
            client
                .get(url)
                .send()
                .await
                .context("HTTP request failed")?
        }
    } else {
        client
            .get(url)
            .send()
            .await
            .context("HTTP request failed")?
    };

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", response.status());
    }

    let total = response.content_length().unwrap_or(size_hint);
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template("  {bar:40.cyan/blue} {bytes}/{total_bytes}  {eta}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(existing_bytes > 0)
        .write(true)
        .truncate(existing_bytes == 0)
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
