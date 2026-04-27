use anyhow::Context;

const KEYRING_SERVICE: &str = "bv-cli";

pub fn resolve_github_token(flag: Option<&str>, non_interactive: bool) -> anyhow::Result<String> {
    if let Some(t) = flag {
        return Ok(t.to_string());
    }
    if let Some(t) = try_gh_auth_token() {
        return Ok(t);
    }
    if let Some(t) = keyring_get("github-token") {
        return Ok(t);
    }
    if non_interactive {
        anyhow::bail!(
            "no GitHub token found\n  \
             Set GITHUB_TOKEN, pass --github-token, or run: gh auth login"
        );
    }
    let token = prompt_token("GitHub personal access token (needs repo + write:packages scopes)")?;
    if dialoguer::Confirm::new()
        .with_prompt("Save token to OS keychain for future use?")
        .default(true)
        .interact()
        .unwrap_or(false)
    {
        keyring_set("github-token", &token);
    }
    Ok(token)
}

pub fn resolve_ghcr_token(flag: Option<&str>, github_token: &str) -> String {
    if let Some(t) = flag {
        return t.to_string();
    }
    if let Ok(t) = std::env::var("GHCR_TOKEN")
        && !t.is_empty()
    {
        return t;
    }
    if let Some(t) = keyring_get("ghcr-token") {
        return t;
    }
    github_token.to_string()
}

fn try_gh_auth_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if out.status.success() {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    } else {
        None
    }
}

fn keyring_get(key: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, key).ok()?;
    match entry.get_password() {
        Ok(t) if !t.is_empty() => Some(t),
        _ => None,
    }
}

fn keyring_set(key: &str, token: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
        let _ = entry.set_password(token);
    }
}

fn prompt_token(prompt: &str) -> anyhow::Result<String> {
    dialoguer::Password::new()
        .with_prompt(prompt)
        .interact()
        .context("failed to read token from terminal")
}
