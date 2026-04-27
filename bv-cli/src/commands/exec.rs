use anyhow::Context;

use bv_core::project::BvLock;

pub fn run(command: &str, args: &[String]) -> anyhow::Result<()> {
    let args: &[String] = args
        .first()
        .filter(|a| a.as_str() == "--")
        .map(|_| &args[1..])
        .unwrap_or(args);

    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!("no bv.lock found\n  run `bv add <tool>` first");
    }

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
