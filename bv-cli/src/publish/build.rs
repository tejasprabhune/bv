use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

/// Log in to GHCR, build the image, push it, and return the resolved digest.
pub fn build_and_push(
    context_dir: &Path,
    dockerfile: &Path,
    image_ref: &str,
    ghcr_token: &str,
    github_username: &str,
    platform: &str,
) -> anyhow::Result<String> {
    docker_login(ghcr_token, github_username)?;
    docker_build(context_dir, dockerfile, image_ref, platform)?;
    docker_push(image_ref)?;
    resolve_digest(image_ref)
}

fn docker_login(token: &str, username: &str) -> anyhow::Result<()> {
    eprintln!(
        "  {} ghcr.io as {}",
        "Logging in".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        username
    );
    let mut child = Command::new("docker")
        .args(["login", "ghcr.io", "-u", username, "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn 'docker login'")?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(token.as_bytes())
        .context("failed to write token to docker login")?;

    let out = child.wait_with_output().context("docker login failed")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("docker login failed: {}", stderr.trim());
    }
    Ok(())
}

fn docker_build(
    context_dir: &Path,
    dockerfile: &Path,
    image_ref: &str,
    platform: &str,
) -> anyhow::Result<()> {
    eprintln!(
        "  {} {} for linux/{}",
        "Building".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        image_ref,
        platform
    );
    let status = Command::new("docker")
        .args([
            "build",
            "--platform",
            &format!("linux/{}", platform),
            "-f",
            &dockerfile.to_string_lossy(),
            "-t",
            image_ref,
            &context_dir.to_string_lossy(),
        ])
        .status()
        .context("failed to spawn 'docker build'")?;

    if !status.success() {
        anyhow::bail!("docker build failed (exit {})", status);
    }
    Ok(())
}

fn docker_push(image_ref: &str) -> anyhow::Result<()> {
    eprintln!(
        "  {} {}",
        "Pushing".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        image_ref
    );
    // Tee stderr through a reader thread so the user still sees live progress
    // while we keep a copy to inspect for known error patterns afterwards.
    let mut child = Command::new("docker")
        .args(["push", image_ref])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn 'docker push'")?;
    let stderr = child.stderr.take().expect("piped stderr");
    let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let captured_w = std::sync::Arc::clone(&captured);
    let pump = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(|l| l.ok()) {
            let _ = writeln!(std::io::stderr(), "{line}");
            if let Ok(mut buf) = captured_w.lock() {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
    });

    let status = child.wait().context("failed to wait on 'docker push'")?;
    let _ = pump.join();
    if status.success() {
        return Ok(());
    }

    let log = captured.lock().map(|s| s.clone()).unwrap_or_default();
    let hint = push_hint(&log);
    if hint.is_empty() {
        anyhow::bail!("docker push failed (exit {status})");
    }
    anyhow::bail!("docker push failed (exit {status})\n{hint}");
}

/// Pattern-match the captured `docker push` stderr for known failure modes
/// and return a focused hint. Empty string means "no specific guidance".
fn push_hint(log: &str) -> String {
    let l = log.to_lowercase();
    if l.contains("permission_denied") && l.contains("scope") {
        return "  hint: your token is missing the `write:packages` scope.\n  \
                Generate one at https://github.com/settings/tokens/new?scopes=repo,write:packages&description=bv-publish\n  \
                Then either:\n    \
                    - export GITHUB_TOKEN=<token> && retry, or\n    \
                    - run: gh auth refresh -h github.com -s write:packages,read:packages"
            .into();
    }
    if l.contains("denied: ") || l.contains("unauthorized") {
        return "  hint: docker is logged in but the registry refused the push.\n  \
                Check that your token has `write:packages` and that you can write to this namespace.\n  \
                See https://github.com/settings/tokens for token scopes."
            .into();
    }
    if l.contains("manifest blob unknown") || l.contains("manifest invalid") {
        return "  hint: the image built but the registry rejected its manifest.\n  \
                Try `docker buildx build --platform linux/amd64 ...` or pass --platform to bv publish."
            .into();
    }
    String::new()
}

fn resolve_digest(image_ref: &str) -> anyhow::Result<String> {
    let out = Command::new("docker")
        .args(["inspect", "--format", "{{index .RepoDigests 0}}", image_ref])
        .output()
        .context("failed to inspect image")?;

    if out.status.success() {
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if let Some(digest) = extract_digest(&raw) {
            return Ok(digest);
        }
    }

    // Fallback: docker manifest inspect
    manifest_digest(image_ref)
}

fn manifest_digest(image_ref: &str) -> anyhow::Result<String> {
    let out = Command::new("docker")
        .args(["manifest", "inspect", "--verbose", image_ref])
        .output()
        .context("docker manifest inspect failed")?;

    if !out.status.success() {
        anyhow::bail!(
            "could not resolve digest for {}: {}",
            image_ref,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&text).context("failed to parse manifest inspect output")?;

    let digest = v["Descriptor"]["digest"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("digest not found in manifest inspect output"))?
        .to_string();

    Ok(digest)
}

fn extract_digest(repo_digest: &str) -> Option<String> {
    let at = repo_digest.find('@')?;
    Some(repo_digest[at + 1..].to_string())
}
