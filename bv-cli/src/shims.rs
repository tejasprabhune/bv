use std::fs;
use std::path::Path;

use bv_core::lockfile::Lockfile;

const SHIM_SCRIPT: &str = "#!/bin/sh\nexec bv run \"$(basename \"$0\")\" \"$@\"\n";

/// Write one shim per binary in `lockfile.binary_index` into `<project>/.bv/bin/`.
///
/// The directory is rebuilt atomically (write to `.bv/bin.tmp/`, then rename)
/// so a partial write is never visible. Stale shims from removed tools are
/// automatically removed because the whole directory is replaced.
pub fn write_shims(project_dir: &Path, lockfile: &Lockfile) -> anyhow::Result<()> {
    let bv_dir = project_dir.join(".bv");
    let bin_dir = bv_dir.join("bin");
    let tmp_dir = bv_dir.join("bin.tmp");

    fs::create_dir_all(&bv_dir)?;

    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    for binary in lockfile.binary_index.keys() {
        let shim_path = tmp_dir.join(binary);
        fs::write(&shim_path, SHIM_SCRIPT)?;
        set_executable(&shim_path)?;
    }

    if bin_dir.exists() {
        fs::remove_dir_all(&bin_dir)?;
    }
    fs::rename(&tmp_dir, &bin_dir)?;

    ensure_gitignore(project_dir)?;

    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn ensure_gitignore(project_dir: &Path) -> anyhow::Result<()> {
    let gitignore_path = project_dir.join(".gitignore");
    let entry = ".bv/";

    if gitignore_path.exists() {
        let content = fs::read_to_string(&gitignore_path)?;
        if content
            .lines()
            .any(|l| l.trim() == entry || l.trim() == ".bv")
        {
            return Ok(());
        }
        let mut file = fs::OpenOptions::new().append(true).open(&gitignore_path)?;
        use std::io::Write as _;
        writeln!(file, "{entry}")?;
    } else {
        fs::write(&gitignore_path, format!("{entry}\n"))?;
    }

    Ok(())
}
