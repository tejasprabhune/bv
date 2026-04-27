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
    let status = Command::new("docker")
        .args(["push", image_ref])
        .status()
        .context("failed to spawn 'docker push'")?;

    if !status.success() {
        anyhow::bail!("docker push failed (exit {})", status);
    }
    Ok(())
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
