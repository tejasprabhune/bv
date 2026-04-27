use anyhow::Context;

use bv_core::project::{BvLock, BvToml};

pub fn run(shell_override: Option<&str>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!("no bv.lock found\n  run `bv add <tool>` first");
    }

    let _lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    let shim_dir = cwd.join(".bv").join("bin");
    if !shim_dir.exists() {
        anyhow::bail!("shim directory not found\n  run `bv sync` to regenerate shims");
    }

    let shell = shell_override
        .map(str::to_string)
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());

    let project_name = BvToml::from_path(&cwd.join("bv.toml"))
        .map(|t| t.project.name)
        .unwrap_or_else(|_| {
            cwd.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "bv".into())
        });

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![shim_dir];
    paths.extend(std::env::split_paths(&path_var));
    let new_path = std::env::join_paths(paths).context("failed to build PATH")?;

    let shell_name = std::path::Path::new(&shell)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sh".into());

    let mut cmd = std::process::Command::new(&shell);
    cmd.env("PATH", &new_path)
        .env("BV_ACTIVE", &project_name)
        .env("BV_PROJECT_ROOT", &cwd);

    match shell_name.as_str() {
        "fish" => {
            // Copy the current fish_prompt, then prepend the bv indicator.
            cmd.arg("--init-command").arg(format!(
                "functions --copy fish_prompt __bv_orig_fish_prompt 2>/dev/null; \
                 function fish_prompt; echo -n '(bv:{project_name}) '; __bv_orig_fish_prompt 2>/dev/null; end"
            ));
        }
        _ => {
            // Use PROMPT_COMMAND so our prefix survives conda/virtualenv PS1 resets.
            // On every prompt: if the prefix is absent, prepend it.
            let prefix = format!("(bv:{project_name}) ");
            let prepend = format!(r#"[ "${{PS1#{prefix}}}" = "$PS1" ] && PS1="{prefix}$PS1""#);
            let existing_pc = std::env::var("PROMPT_COMMAND").unwrap_or_default();
            let new_pc = if existing_pc.is_empty() {
                prepend
            } else {
                format!("{prepend}; {existing_pc}")
            };
            cmd.env("PROMPT_COMMAND", new_pc);
        }
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn shell '{shell}'"))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
