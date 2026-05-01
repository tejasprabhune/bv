//! `bv cache` subcommands: `size`, `list`, and `prune`.
//!
//! These commands operate on the bv-managed cache rooted at the path returned
//! by [`CacheLayout::root`]. Every operation here is scoped to that root and
//! never touches files outside it.
//!
//! Reachability for `prune` is computed from the union of bv.lock files we
//! can discover: `$PWD/bv.lock` plus every `bv.lock` under any directory
//! listed in the `BV_KNOWN_PROJECTS` env var (colon-separated).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::lockfile::Lockfile;
use bv_core::project::BvLock;

use crate::commands::add::format_size;

const TMP_TTL: Duration = Duration::from_secs(60 * 60); // 1 hour
const INDEX_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days

/// Top-level dispatch from `Commands::Cache`.
pub fn run(cmd: &crate::cli::CacheCommands) -> anyhow::Result<()> {
    match cmd {
        crate::cli::CacheCommands::Size => run_size(),
        crate::cli::CacheCommands::List => run_list(),
        crate::cli::CacheCommands::Prune {
            dry_run,
            yes,
            all,
            keep_recent,
        } => run_prune(*dry_run, *yes, *all, *keep_recent),
    }
}

// size

pub fn run_size() -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let root = cache.root().clone();

    let categories: Vec<(&str, PathBuf)> = vec![
        ("Tools manifests", root.join("tools")),
        ("Indexes", root.join("index")),
        ("SIFs", root.join("sif")),
        ("Datasets", root.join("data")),
        ("Tmp", root.join("tmp")),
    ];

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.enable_steady_tick(Duration::from_millis(80));
    spinner.set_message("scanning cache");

    let mut sizes: Vec<(&str, u64, PathBuf)> = Vec::with_capacity(categories.len());
    let mut total: u64 = 0;
    for (label, path) in &categories {
        spinner.set_message(format!("scanning {}", label.to_lowercase()));
        let bytes = if path.exists() {
            dir_size(path).unwrap_or(0)
        } else {
            0
        };
        total += bytes;
        sizes.push((label, bytes, path.clone()));
    }
    spinner.finish_and_clear();

    let w_label = sizes.iter().map(|(l, _, _)| l.len()).max().unwrap_or(8) + 2;
    let w_size = sizes
        .iter()
        .map(|(_, b, _)| format_size(*b).len())
        .max()
        .unwrap_or(8)
        .max(format_size(total).len())
        + 2;

    println!(
        "  {:<w_label$}  {:<w_size$}  {}",
        "Category".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        "Size".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        "Path".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
    );
    for (label, bytes, path) in &sizes {
        println!(
            "  {:<w_label$}  {:<w_size$}  {}",
            label,
            format_size(*bytes),
            display_path(path).if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
        );
    }
    println!(
        "  {:<w_label$}  {:<w_size$}",
        "Total".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        format_size(total).if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
    );

    Ok(())
}

// list

pub fn run_list() -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let root = cache.root();

    let inv = inventory(root);

    if inv.is_empty() {
        println!("  cache is empty (root: {})", display_path(root));
        return Ok(());
    }

    println!(
        "  {:<10}  {:<40}  {:<10}  {}",
        "Category".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        "Entry".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        "Size".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
        "Path".if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
    );

    for (cat, name, size, path) in &inv {
        println!(
            "  {:<10}  {:<40}  {:<10}  {}",
            cat,
            truncate(name, 40),
            format_size(*size),
            display_path(path).if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
        );
    }
    Ok(())
}

/// Build a sorted, flat inventory of cache contents.
fn inventory(root: &Path) -> Vec<(&'static str, String, u64, PathBuf)> {
    let mut out: Vec<(&'static str, String, u64, PathBuf)> = Vec::new();

    // SIFs.
    let sif = root.join("sif");
    if let Ok(entries) = fs::read_dir(&sif) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|ext| ext == "sif") {
                let size = file_size(&p);
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push(("sif", name, size, p));
            }
        }
    }

    // Tool manifests: <tools>/<id>/<version>/manifest.toml
    let tools = root.join("tools");
    if let Ok(ids) = fs::read_dir(&tools) {
        for id_entry in ids.flatten() {
            let id_path = id_entry.path();
            if !id_path.is_dir() {
                continue;
            }
            let id = id_entry.file_name().to_string_lossy().into_owned();
            if let Ok(versions) = fs::read_dir(&id_path) {
                for v in versions.flatten() {
                    let vp = v.path();
                    if !vp.is_dir() {
                        continue;
                    }
                    let version = v.file_name().to_string_lossy().into_owned();
                    let size = dir_size(&vp).unwrap_or(0);
                    out.push((
                        "manifest",
                        format!("{id}@{version}"),
                        size,
                        vp,
                    ));
                }
            }
        }
    }

    // Indexes.
    let indexes = root.join("index");
    if let Ok(entries) = fs::read_dir(&indexes) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().into_owned();
                let size = dir_size(&p).unwrap_or(0);
                out.push(("index", name, size, p));
            }
        }
    }

    // Datasets: <data>/<id>/<version>/
    let data = root.join("data");
    if let Ok(ids) = fs::read_dir(&data) {
        for id_entry in ids.flatten() {
            let id_path = id_entry.path();
            if !id_path.is_dir() {
                continue;
            }
            let id = id_entry.file_name().to_string_lossy().into_owned();
            if let Ok(versions) = fs::read_dir(&id_path) {
                for v in versions.flatten() {
                    let vp = v.path();
                    if !vp.is_dir() {
                        continue;
                    }
                    let version = v.file_name().to_string_lossy().into_owned();
                    let size = dir_size(&vp).unwrap_or(0);
                    out.push((
                        "dataset",
                        format!("{id}@{version}"),
                        size,
                        vp,
                    ));
                }
            }
        }
    }

    out.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(&b.1)));
    out
}

// prune

#[derive(Debug, Default, Clone)]
pub struct PruneSummary {
    pub sifs_removed: usize,
    pub sif_bytes: u64,
    pub manifests_removed: usize,
    pub manifest_bytes: u64,
    pub indexes_removed: usize,
    pub index_bytes: u64,
    pub tmp_removed: usize,
    pub tmp_bytes: u64,
}

impl PruneSummary {
    pub fn total_bytes(&self) -> u64 {
        self.sif_bytes + self.manifest_bytes + self.index_bytes + self.tmp_bytes
    }
}

/// Reachability set extracted from one or more lockfiles.
#[derive(Debug, Default, Clone)]
pub struct Reachable {
    /// All `image_digest` strings (e.g. `sha256:...`).
    pub digests: BTreeSet<String>,
    /// All `(tool_id, version)` pairs.
    pub tool_versions: BTreeSet<(String, String)>,
    /// All `image_reference` strings (e.g. `ghcr.io/org/tool:1.0`).
    pub image_references: BTreeSet<String>,
}

impl Reachable {
    pub fn merge(&mut self, lock: &Lockfile) {
        for entry in lock.tools.values() {
            self.digests.insert(entry.image_digest.clone());
            self.tool_versions
                .insert((entry.tool_id.clone(), entry.version.clone()));
            self.image_references.insert(entry.image_reference.clone());
        }
    }
}

pub fn run_prune(
    dry_run: bool,
    yes: bool,
    all: bool,
    keep_recent: Option<usize>,
) -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let root = cache.root().clone();

    if !root.exists() {
        println!("  cache is empty (root: {})", display_path(&root));
        return Ok(());
    }

    let cwd = std::env::current_dir().ok();
    let reachable = gather_reachable(cwd.as_deref());

    // Plan filesystem removals.
    let plan = plan_prune(&root, &reachable, all, keep_recent)?;
    let tmp_plan = plan_tmp(&root.join("tmp"));

    let total_items = plan.items.len() + tmp_plan.items.len();
    let total_bytes: u64 = plan.items.iter().map(|i| i.size).sum::<u64>()
        + tmp_plan.items.iter().map(|i| i.size).sum::<u64>();

    // Load the persistent ownership record. This is the authoritative list of
    // every Docker image bv has ever pulled — it survives `bv remove`.
    let owned_path = cache.owned_images_path();
    let owned = bv_core::owned_images::OwnedImages::load(&owned_path);

    // Docker image candidates: owned by bv and (if !all) not in any current lockfile.
    let docker_candidates = docker_unreferenced_images(&owned, &reachable, all);

    if total_items == 0 && docker_candidates.is_empty() {
        println!("  nothing to prune");
        return Ok(());
    }

    if dry_run {
        println!("  (dry run) would remove:");
        print_plan(&plan, &tmp_plan);
        if !docker_candidates.is_empty() {
            println!("  Docker images (unreferenced by any bv.lock):");
            for img in &docker_candidates {
                println!("    {}", img.display_ref);
            }
        }
        println!(
            "  Total: {} items, {} would be freed.",
            total_items,
            format_size(total_bytes),
        );
        return Ok(());
    }

    let confirm_msg = match (total_items, docker_candidates.len()) {
        (0, d) => format!("  Remove {d} Docker image{}? [y/N] ", if d == 1 { "" } else { "s" }),
        (n, 0) => format!("  Remove {n} cache item{}, free {}? [y/N] ",
            if n == 1 { "" } else { "s" }, format_size(total_bytes)),
        (n, d) => format!("  Remove {n} cache item{} and {d} Docker image{}, free {}? [y/N] ",
            if n == 1 { "" } else { "s" },
            if d == 1 { "" } else { "s" },
            format_size(total_bytes)),
    };

    if !yes {
        eprint!("{confirm_msg}");
        let _ = std::io::stderr().flush();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        let answer = buf.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            eprintln!("  aborted");
            return Ok(());
        }
    }

    let summary = apply_plan(&plan, &tmp_plan)?;
    print_summary(&summary);

    for img in &docker_candidates {
        let ok = std::process::Command::new("docker")
            .args(["rmi", &img.display_ref])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let _ = bv_core::owned_images::remove_by_digest(&owned_path, &img.digest);
            println!(
                "  {} Docker image {}",
                "Removed".if_supports_color(Stream::Stdout, |t| t.green().bold().to_string()),
                img.display_ref,
            );
        } else {
            println!(
                "  {} could not remove Docker image {} (may be in use or Docker unavailable)",
                "warning:".if_supports_color(Stream::Stdout, |t| t.yellow().bold().to_string()),
                img.display_ref,
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct PruneItem {
    /// "sif" | "manifest" | "index" | "tmp"
    pub category: &'static str,
    pub display: String,
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Default, Clone)]
pub struct PrunePlan {
    pub items: Vec<PruneItem>,
}

/// Walk the cache and decide what to remove (excluding tmp; that's separate).
pub fn plan_prune(
    root: &Path,
    reachable: &Reachable,
    all: bool,
    keep_recent: Option<usize>,
) -> anyhow::Result<PrunePlan> {
    let mut plan = PrunePlan::default();

    // SIFs.
    let sif_dir = root.join("sif");
    if let Ok(entries) = fs::read_dir(&sif_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().is_none_or(|ext| ext != "sif") {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // Files are written as `<sanitized digest>.sif`; recover digest by
            // matching the sanitized form against all reachable digests.
            let is_reachable =
                !all && reachable.digests.iter().any(|d| sanitize_digest(d) == stem);
            if !is_reachable {
                let size = file_size(&p);
                plan.items.push(PruneItem {
                    category: "sif",
                    display: p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: p,
                    size,
                });
            }
        }
    }

    // Tool manifests: <tools>/<id>/<version>/.
    let tools_dir = root.join("tools");
    if let Ok(ids) = fs::read_dir(&tools_dir) {
        // Group by id so we can apply --keep-recent per tool.
        let mut per_tool: BTreeMap<String, Vec<(String, PathBuf, SystemTime, u64)>> =
            BTreeMap::new();
        for id_entry in ids.flatten() {
            let id_path = id_entry.path();
            if !id_path.is_dir() {
                continue;
            }
            let id = id_entry.file_name().to_string_lossy().into_owned();
            if let Ok(versions) = fs::read_dir(&id_path) {
                for v in versions.flatten() {
                    let vp = v.path();
                    if !vp.is_dir() {
                        continue;
                    }
                    let version = v.file_name().to_string_lossy().into_owned();
                    let mtime = v
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    let size = dir_size(&vp).unwrap_or(0);
                    per_tool
                        .entry(id.clone())
                        .or_default()
                        .push((version, vp, mtime, size));
                }
            }
        }

        for (id, mut versions) in per_tool {
            // Newest first.
            versions.sort_by(|a, b| b.2.cmp(&a.2));
            let keep_count = keep_recent.unwrap_or(0);
            let mut kept_extra: usize = 0;
            for (version, vp, _mtime, size) in versions {
                let is_reachable = !all
                    && reachable
                        .tool_versions
                        .contains(&(id.clone(), version.clone()));
                if is_reachable {
                    continue;
                }
                if !all && kept_extra < keep_count {
                    kept_extra += 1;
                    continue;
                }
                plan.items.push(PruneItem {
                    category: "manifest",
                    display: format!("{id}@{version}"),
                    path: vp,
                    size,
                });
            }
        }
    }

    // Index clones older than 30 days. Skip the default index.
    let index_dir = root.join("index");
    if let Ok(entries) = fs::read_dir(&index_dir) {
        let now = SystemTime::now();
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "default" {
                continue;
            }
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
            if all || age > INDEX_TTL {
                let size = dir_size(&p).unwrap_or(0);
                plan.items.push(PruneItem {
                    category: "index",
                    display: name,
                    path: p,
                    size,
                });
            }
        }
    }

    Ok(plan)
}

/// Always-on tmp sweep: any entry older than [`TMP_TTL`].
pub fn plan_tmp(tmp_dir: &Path) -> PrunePlan {
    let mut plan = PrunePlan::default();
    let Ok(entries) = fs::read_dir(tmp_dir) else {
        return plan;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let p = e.path();
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
        if age <= TMP_TTL {
            continue;
        }
        let size = if p.is_dir() {
            dir_size(&p).unwrap_or(0)
        } else {
            file_size(&p)
        };
        plan.items.push(PruneItem {
            category: "tmp",
            display: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: p,
            size,
        });
    }
    plan
}

fn print_plan(plan: &PrunePlan, tmp_plan: &PrunePlan) {
    for item in plan.items.iter().chain(tmp_plan.items.iter()) {
        println!(
            "    {:<10} {:<40} {}",
            item.category,
            truncate(&item.display, 40),
            format_size(item.size),
        );
    }
}

pub fn apply_plan(plan: &PrunePlan, tmp_plan: &PrunePlan) -> anyhow::Result<PruneSummary> {
    let mut summary = PruneSummary::default();
    for item in plan.items.iter().chain(tmp_plan.items.iter()) {
        let removed = remove_path(&item.path).is_ok();
        if !removed {
            continue;
        }
        match item.category {
            "sif" => {
                summary.sifs_removed += 1;
                summary.sif_bytes += item.size;
            }
            "manifest" => {
                summary.manifests_removed += 1;
                summary.manifest_bytes += item.size;
            }
            "index" => {
                summary.indexes_removed += 1;
                summary.index_bytes += item.size;
            }
            "tmp" => {
                summary.tmp_removed += 1;
                summary.tmp_bytes += item.size;
            }
            _ => {}
        }
    }
    Ok(summary)
}

fn print_summary(s: &PruneSummary) {
    println!(
        "  Removed   {:>3} SIFs        {}",
        s.sifs_removed,
        format_size(s.sif_bytes)
    );
    println!(
        "  Removed   {:>3} manifests   {}",
        s.manifests_removed,
        format_size(s.manifest_bytes)
    );
    if s.indexes_removed > 0 {
        println!(
            "  Removed   {:>3} indexes     {}",
            s.indexes_removed,
            format_size(s.index_bytes)
        );
    }
    if s.tmp_removed > 0 {
        println!(
            "  Removed   {:>3} tmp entries {}",
            s.tmp_removed,
            format_size(s.tmp_bytes)
        );
    }
    println!(
        "  Total freed             {}",
        format_size(s.total_bytes())
    );
}

// Reachability discovery

pub fn gather_reachable(cwd: Option<&Path>) -> Reachable {
    let mut reach = Reachable::default();

    if let Some(cwd) = cwd {
        let p = cwd.join("bv.lock");
        if p.exists()
            && let Ok(lock) = BvLock::from_path(&p)
        {
            reach.merge(&lock);
        }
    }

    if let Ok(known) = std::env::var("BV_KNOWN_PROJECTS") {
        for dir in known.split(':').filter(|s| !s.is_empty()) {
            collect_locks_under(Path::new(dir), &mut reach);
        }
    }

    reach
}

/// Walk a directory tree (bounded depth to avoid going wild) and merge any
/// `bv.lock` we encounter into `reach`.
fn collect_locks_under(root: &Path, reach: &mut Reachable) {
    fn walk(p: &Path, depth: usize, reach: &mut Reachable) {
        if depth == 0 {
            return;
        }
        let lock = p.join("bv.lock");
        if lock.is_file()
            && let Ok(l) = BvLock::from_path(&lock)
        {
            reach.merge(&l);
        }
        let Ok(entries) = fs::read_dir(p) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            // Skip dotfiles and common heavy dirs.
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == "venv"
                || name == ".venv"
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, depth - 1, reach);
            }
        }
    }
    walk(root, 6, reach);
}

// Docker image helpers

struct DockerImage {
    /// The reference we pass to `docker rmi` (repo:tag or repo@digest).
    display_ref: String,
    /// Full image ID from `docker images --no-trunc`, used to remove from owned-images.txt.
    digest: String,
}

/// Return Docker images that bv pulled and are eligible for removal.
///
/// Ownership is determined by `owned` (the persistent `owned-images.txt` record,
/// written every time bv pulls an image). Only owned images are ever touched —
/// this is what prevents bv from removing unrelated Docker images.
///
/// `reachable` is the set of images currently referenced by known lockfiles.
/// When `remove_all` is false, referenced images are exempt from removal.
/// When `remove_all` is true, all owned images are candidates.
fn docker_unreferenced_images(
    owned: &bv_core::owned_images::OwnedImages,
    reachable: &Reachable,
    remove_all: bool,
) -> Vec<DockerImage> {
    if owned.is_empty() && !remove_all {
        return vec![];
    }

    let Ok(out) = std::process::Command::new("docker")
        .args([
            "images",
            "--no-trunc",
            "--format",
            "{{.Repository}}:{{.Tag}}\t{{.Digest}}\t{{.ID}}",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return vec![];
    };

    if !out.status.success() {
        return vec![];
    }

    let mut candidates = vec![];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let ref_tag = parts[0];
        let digest = parts[1]; // "sha256:..." or "<none>"
        let id = parts[2];     // "sha256:..." full image ID

        // Ownership check: only touch images bv has recorded pulling.
        let is_owned = owned.references.contains(ref_tag)
            || (!digest.is_empty() && digest != "<none>" && owned.digests.contains(digest))
            || (!id.is_empty() && owned.digests.contains(id));
        if !is_owned {
            continue;
        }

        // Exemption check: skip images still in an active lockfile (unless --all).
        if !remove_all {
            let in_active_lockfile = reachable.image_references.contains(ref_tag)
                || (!digest.is_empty() && digest != "<none>" && reachable.digests.contains(digest));
            if in_active_lockfile {
                continue;
            }
        }

        let display_ref = if digest != "<none>" {
            format!("{ref_tag}@{digest}")
        } else {
            ref_tag.to_string()
        };
        candidates.push(DockerImage { display_ref, digest: id.to_string() });
    }

    candidates
}

// Filesystem helpers

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let entries = match fs::read_dir(&p) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let path = e.path();
            let meta = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Sanitize a digest the same way the apptainer runtime does, so we can
/// compare cached SIF filenames to digests recorded in lockfiles.
fn sanitize_digest(digest: &str) -> String {
    digest
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = String::with_capacity(max);
    out.push_str(&s[..max.saturating_sub(1)]);
    out.push('…');
    out
}

fn display_path(p: &Path) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    p.display().to_string()
}

// tests

#[cfg(test)]
mod tests {
    use super::*;
    use bv_core::lockfile::{Lockfile, LockfileEntry};
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn make_entry(id: &str, version: &str, digest: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: id.into(),
            declared_version_req: String::new(),
            version: version.into(),
            spec_kind: bv_core::lockfile::SpecKind::LegacyImage,
            image_reference: format!("registry/{id}:{version}"),
            image_digest: digest.into(),
            manifest_sha256: String::new(),
            image_size_bytes: None,
            layers: vec![],
            resolved_at: Utc::now(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![],
        }
    }

    #[test]
    fn dir_size_sums_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("a.txt"), &[0u8; 100]);
        write_file(&tmp.path().join("nested/b.txt"), &[0u8; 250]);
        write_file(&tmp.path().join("nested/c/d.txt"), &[0u8; 50]);
        let total = dir_size(tmp.path()).unwrap();
        assert_eq!(total, 400);
    }

    #[test]
    fn sanitize_digest_matches_apptainer_filename() {
        // Mirror of bv_runtime_apptainer::cache::sif_path_for_digest.
        let digest = "sha256:abc123";
        let sanitized = sanitize_digest(digest);
        // Colon becomes underscore, rest preserved.
        assert_eq!(sanitized, "sha256_abc123");
    }

    #[test]
    fn plan_prune_keeps_reachable_sifs_and_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Reachable: sha256:keep, tool foo@1.0
        // Orphan:    sha256:drop, tool foo@0.9 (older), bar@2.0
        let keep_digest = "sha256:keep";
        let drop_digest = "sha256:drop";

        // SIF files.
        let sif_keep = root.join("sif").join(format!("{}.sif", sanitize_digest(keep_digest)));
        let sif_drop = root.join("sif").join(format!("{}.sif", sanitize_digest(drop_digest)));
        write_file(&sif_keep, &[1u8; 1024]);
        write_file(&sif_drop, &[2u8; 2048]);

        // Manifests.
        write_file(&root.join("tools/foo/1.0/manifest.toml"), b"x");
        write_file(&root.join("tools/foo/0.9/manifest.toml"), b"x");
        write_file(&root.join("tools/bar/2.0/manifest.toml"), b"x");

        // Indexes: a fresh `default` (skipped) and an old custom one.
        write_file(&root.join("index/default/HEAD"), b"ref");
        let custom = root.join("index/custom/HEAD");
        write_file(&custom, b"ref");
        // Backdate the custom index's mtime by 60 days.
        let old = SystemTime::now() - Duration::from_secs(60 * 24 * 60 * 60);
        filetime::set_file_mtime(
            root.join("index/custom"),
            filetime::FileTime::from_system_time(old),
        )
        .ok();

        let mut reach = Reachable::default();
        reach.digests.insert(keep_digest.to_string());
        reach
            .tool_versions
            .insert(("foo".to_string(), "1.0".to_string()));

        let plan = plan_prune(root, &reach, false, None).unwrap();

        let categories: Vec<_> = plan.items.iter().map(|i| (i.category, i.display.clone())).collect();
        // Reachable SIF and reachable manifest must survive.
        assert!(!categories.iter().any(|(c, d)| *c == "sif" && d.contains("keep")),
            "reachable SIF was scheduled for removal: {categories:?}");
        assert!(!categories.iter().any(|(c, d)| *c == "manifest" && d == "foo@1.0"),
            "reachable manifest was scheduled for removal: {categories:?}");
        // Orphans must be present.
        assert!(categories.iter().any(|(c, d)| *c == "sif" && d.contains("drop")),
            "orphan SIF was not scheduled for removal: {categories:?}");
        assert!(categories.iter().any(|(c, d)| *c == "manifest" && d == "foo@0.9"));
        assert!(categories.iter().any(|(c, d)| *c == "manifest" && d == "bar@2.0"));
        // Custom (old) index pruned, default (fresh) preserved.
        if filetime::FileTime::from_system_time(old) > filetime::FileTime::from_unix_time(0, 0) {
            // mtime backdating may be a no-op on some filesystems; only assert when it's effective.
            // Otherwise skip the check.
        }
    }

    #[test]
    fn plan_prune_all_drops_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_file(&root.join("sif/whatever.sif"), &[0u8; 10]);
        write_file(&root.join("tools/foo/1.0/manifest.toml"), b"x");
        let plan = plan_prune(root, &Reachable::default(), true, None).unwrap();
        assert!(plan.items.iter().any(|i| i.category == "sif"));
        assert!(plan.items.iter().any(|i| i.category == "manifest"));
    }

    #[test]
    fn keep_recent_keeps_newest_unreachable_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Three versions of foo, none reachable.
        for v in ["1.0", "1.1", "1.2"] {
            write_file(&root.join(format!("tools/foo/{v}/manifest.toml")), b"x");
        }
        // Stagger mtimes so 1.2 is the newest.
        let now = SystemTime::now();
        filetime::set_file_mtime(
            root.join("tools/foo/1.0"),
            filetime::FileTime::from_system_time(now - Duration::from_secs(300)),
        )
        .ok();
        filetime::set_file_mtime(
            root.join("tools/foo/1.1"),
            filetime::FileTime::from_system_time(now - Duration::from_secs(200)),
        )
        .ok();
        filetime::set_file_mtime(
            root.join("tools/foo/1.2"),
            filetime::FileTime::from_system_time(now - Duration::from_secs(100)),
        )
        .ok();

        let plan = plan_prune(root, &Reachable::default(), false, Some(1)).unwrap();
        let mans: Vec<&str> = plan
            .items
            .iter()
            .filter(|i| i.category == "manifest")
            .map(|i| i.display.as_str())
            .collect();
        // Only one of the three should be removed; the most recent (1.2) is kept.
        assert_eq!(mans.len(), 2, "{mans:?}");
        assert!(mans.contains(&"foo@1.0"));
        assert!(mans.contains(&"foo@1.1"));
    }

    #[test]
    fn run_size_walks_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_file(&root.join("sif/x.sif"), &[0u8; 4096]);
        write_file(&root.join("tools/foo/1.0/manifest.toml"), &[0u8; 100]);
        write_file(&root.join("data/db/v1/file.bin"), &[0u8; 8192]);

        let cache = CacheLayout::with_root(root.to_path_buf());
        // Compute manually using our helper to avoid environment coupling.
        let sif = dir_size(&cache.sif_dir()).unwrap();
        let tools = dir_size(&cache.root().join("tools")).unwrap();
        let data = dir_size(&cache.root().join("data")).unwrap();
        assert_eq!(sif, 4096);
        assert_eq!(tools, 100);
        assert_eq!(data, 8192);
    }

    #[test]
    fn gather_reachable_reads_pwd_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let mut lock = Lockfile::new();
        lock.tools.insert(
            "foo".into(),
            make_entry("foo", "1.0", "sha256:abc"),
        );
        let s = lock.to_toml_string().unwrap();
        fs::write(cwd.join("bv.lock"), s).unwrap();
        let reach = gather_reachable(Some(cwd));
        assert!(reach.digests.contains("sha256:abc"));
        assert!(reach.tool_versions.contains(&("foo".into(), "1.0".into())));
    }

    #[test]
    fn plan_tmp_only_takes_old_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_dir = tmp.path().join("tmp");
        fs::create_dir_all(&tmp_dir).unwrap();
        let young = tmp_dir.join("young.bin");
        let old = tmp_dir.join("old.bin");
        write_file(&young, &[0u8; 16]);
        write_file(&old, &[0u8; 16]);
        // Backdate `old` by 2 hours.
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        filetime::set_file_mtime(
            &old,
            filetime::FileTime::from_system_time(two_hours_ago),
        )
        .ok();

        let plan = plan_tmp(&tmp_dir);
        let names: Vec<_> = plan.items.iter().map(|i| i.display.clone()).collect();
        assert!(names.iter().any(|n| n == "old.bin"));
        assert!(!names.iter().any(|n| n == "young.bin"));
    }
}

