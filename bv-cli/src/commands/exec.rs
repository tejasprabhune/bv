use std::fs;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::lockfile::Lockfile;
use bv_core::project::BvLock;
use bv_runtime::ContainerRuntime as _;

pub async fn run(command: &str, args: &[String], no_sync: bool) -> anyhow::Result<()> {
    let args: &[String] = args
        .first()
        .filter(|a| a.as_str() == "--")
        .map(|_| &args[1..])
        .unwrap_or(args);

    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");
    let bv_toml_path = cwd.join("bv.toml");

    if !bv_lock_path.exists() {
        anyhow::bail!("no bv.lock found\n  run `bv add <tool>` first");
    }

    let skip_sync = no_sync || env_flag_set("BV_EXEC_NO_SYNC");

    if !skip_sync {
        auto_sync(&cwd, &bv_toml_path, &bv_lock_path)
            .await
            .context("auto-sync failed")?;
    }

    // Re-read the lockfile after a possible sync; it may have changed.
    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    let shim_dir = cwd.join(".bv").join("bin");
    if !shim_dir.exists() {
        if lockfile.binary_index.is_empty() {
            anyhow::bail!(
                "no binaries are available in this project\n  \
                 the installed tools may not declare [tool.binaries] yet"
            );
        }
        anyhow::bail!("shim directory not found\n  run `bv sync` to regenerate shims");
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(&path_var));
    let new_path = std::env::join_paths(paths).context("failed to build PATH")?;

    exec_process(command, args, &new_path, &cwd)
}

/// Auto-sync the project before exec, mirroring `uv run`'s behavior.
///
/// 1. If bv.toml's mtime is newer than bv.lock's, the lockfile is stale:
///    re-resolve via `bv lock` and pull via `bv sync`.
/// 2. Else if any tool image is missing locally, run `bv sync` to fetch.
/// 3. Else if shims are missing/empty but the lockfile has tools, write shims.
/// 4. Else: silent fast path.
async fn auto_sync(
    cwd: &Path,
    bv_toml_path: &Path,
    bv_lock_path: &Path,
) -> anyhow::Result<()> {
    let lockfile = BvLock::from_path(bv_lock_path).context("failed to read bv.lock")?;

    if bv_toml_path.exists() && lockfile_is_stale(bv_toml_path, bv_lock_path)? {
        eprintln!(
            "  {} bv.lock (bv.toml is newer)",
            "Updating".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string())
        );
        crate::commands::lock::run(false, None).await?;
        crate::commands::sync::run(false, None, None).await?;
        return Ok(());
    }

    if lockfile.tools.is_empty() {
        return Ok(());
    }

    if any_image_missing(&lockfile)? {
        crate::commands::sync::run(false, None, None).await?;
        return Ok(());
    }

    if shims_missing_or_empty(cwd)? && !lockfile.binary_index.is_empty() {
        crate::shims::write_shims(cwd, &lockfile)?;
    }

    Ok(())
}

/// True if `bv.toml` was modified after `bv.lock`. If either mtime cannot be
/// read, errs on the safe side and reports the lockfile as stale.
fn lockfile_is_stale(bv_toml_path: &Path, bv_lock_path: &Path) -> anyhow::Result<bool> {
    let toml_mtime = mtime_of(bv_toml_path)?;
    let lock_mtime = mtime_of(bv_lock_path)?;
    Ok(toml_mtime > lock_mtime)
}

fn mtime_of(path: &Path) -> anyhow::Result<SystemTime> {
    let meta = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let mtime = meta
        .modified()
        .with_context(|| format!("failed to read mtime of {}", path.display()))?;
    Ok(mtime)
}

/// True if at least one tool image referenced by the lockfile is not present
/// in the local container store. Selects a runtime via the same logic the
/// other commands use; honors bv.toml's [runtime.backend] when present.
fn any_image_missing(lockfile: &Lockfile) -> anyhow::Result<bool> {
    if lockfile.tools.is_empty() {
        return Ok(false);
    }

    let bv_toml = std::env::current_dir()
        .ok()
        .map(|d| d.join("bv.toml"))
        .and_then(|p| bv_core::project::BvToml::from_path(&p).ok());

    let runtime = crate::runtime_select::resolve_runtime(None, bv_toml.as_ref())?;

    for entry in lockfile.tools.values() {
        let base_ref = crate::ops::base_image_ref(&entry.image_reference);
        if !runtime.is_locally_available(&base_ref, &entry.image_digest) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn shims_missing_or_empty(cwd: &Path) -> anyhow::Result<bool> {
    let bin_dir = cwd.join(".bv").join("bin");
    if !bin_dir.exists() {
        return Ok(true);
    }
    let mut iter = fs::read_dir(&bin_dir)
        .with_context(|| format!("failed to read {}", bin_dir.display()))?;
    Ok(iter.next().is_none())
}

fn env_flag_set(name: &str) -> bool {
    std::env::var_os(name)
        .map(|v| {
            let s = v.to_string_lossy();
            !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

#[cfg(unix)]
fn exec_process(
    command: &str,
    args: &[String],
    new_path: &std::ffi::OsStr,
    project_root: &std::path::Path,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let err = std::process::Command::new(command)
        .args(args)
        .env("PATH", new_path)
        .env("BV_PROJECT_ROOT", project_root)
        .exec();

    Err(anyhow::anyhow!("failed to exec '{command}': {err}"))
}

#[cfg(not(unix))]
fn exec_process(
    command: &str,
    args: &[String],
    new_path: &std::ffi::OsStr,
    project_root: &std::path::Path,
) -> anyhow::Result<()> {
    let status = std::process::Command::new(command)
        .args(args)
        .env("PATH", new_path)
        .env("BV_PROJECT_ROOT", project_root)
        .status()
        .with_context(|| format!("failed to run '{command}'"))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lockfile_stale_when_toml_newer() {
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("bv.toml");
        let lock = dir.path().join("bv.lock");
        fs::write(&lock, "").unwrap();
        // Sleep briefly to guarantee a distinct mtime; filesystem mtime
        // resolution can be coarse (1s on some platforms).
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(&toml, "").unwrap();
        assert!(lockfile_is_stale(&toml, &lock).unwrap());
    }

    #[test]
    fn lockfile_fresh_when_lock_newer() {
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("bv.toml");
        let lock = dir.path().join("bv.lock");
        fs::write(&toml, "").unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(&lock, "").unwrap();
        assert!(!lockfile_is_stale(&toml, &lock).unwrap());
    }

    #[test]
    fn shims_dir_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(shims_missing_or_empty(dir.path()).unwrap());
    }

    #[test]
    fn shims_dir_empty_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".bv").join("bin")).unwrap();
        assert!(shims_missing_or_empty(dir.path()).unwrap());
    }

    #[test]
    fn shims_dir_with_file_is_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(".bv").join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("foo"), "x").unwrap();
        assert!(!shims_missing_or_empty(dir.path()).unwrap());
    }

    #[test]
    fn env_flag_truthy() {
        // Unique var name per-test to avoid cross-test interference.
        let key = "BV_EXEC_NO_SYNC_TEST_TRUE";
        // SAFETY: tests in this module are single-threaded with respect to
        // these vars; no other test reads them.
        unsafe {
            std::env::set_var(key, "1");
        }
        assert!(env_flag_set(key));
        unsafe {
            std::env::set_var(key, "true");
        }
        assert!(env_flag_set(key));
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn env_flag_falsy() {
        let key = "BV_EXEC_NO_SYNC_TEST_FALSE";
        unsafe {
            std::env::remove_var(key);
        }
        assert!(!env_flag_set(key));
        unsafe {
            std::env::set_var(key, "0");
        }
        assert!(!env_flag_set(key));
        unsafe {
            std::env::set_var(key, "false");
        }
        assert!(!env_flag_set(key));
        unsafe {
            std::env::set_var(key, "");
        }
        assert!(!env_flag_set(key));
        unsafe {
            std::env::remove_var(key);
        }
    }
}
