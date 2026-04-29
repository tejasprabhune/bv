use std::path::{Path, PathBuf};

use anyhow::Context;
use tempfile::TempDir;

pub struct FetchedSource {
    pub dir: PathBuf,
    pub name_hint: String,
    pub version_hint: Option<String>,
    pub source_url: String,
    _tempdir: Option<TempDir>,
}

pub enum Source {
    LocalDir(PathBuf),
    GitHub {
        owner: String,
        repo: String,
        git_ref: Option<String>,
    },
}

impl Source {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        // Accept three GitHub forms:
        //   github:owner/repo[@ref]
        //   https://github.com/owner/repo[.git][/tree/<ref>]
        //   git@github.com:owner/repo[.git]
        let github_part = if let Some(rest) = spec.strip_prefix("github:") {
            Some(rest.to_string())
        } else if let Some(rest) = spec
            .strip_prefix("https://github.com/")
            .or_else(|| spec.strip_prefix("http://github.com/"))
        {
            // Strip trailing .git, /, or /tree/<ref>
            let (path, ref_from_url) = if let Some((p, r)) = rest.split_once("/tree/") {
                (p, Some(r.trim_end_matches('/').to_string()))
            } else {
                (rest.trim_end_matches('/'), None)
            };
            let path = path.trim_end_matches(".git");
            Some(ref_from_url.map_or_else(|| path.to_string(), |r| format!("{path}@{r}")))
        } else {
            spec.strip_prefix("git@github.com:")
                .map(|rest| rest.trim_end_matches(".git").to_string())
        };

        if let Some(gh) = github_part {
            let (repo_part, git_ref) = gh
                .split_once('@')
                .map(|(rp, r)| (rp, Some(r.to_string())))
                .unwrap_or((gh.as_str(), None));
            let (owner, repo) = repo_part.split_once('/').ok_or_else(|| {
                anyhow::anyhow!(
                    "github source must look like 'github:owner/repo' or \
                     'https://github.com/owner/repo', got '{}'",
                    spec
                )
            })?;
            Ok(Source::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                git_ref,
            })
        } else {
            let path = PathBuf::from(spec);
            let canonical = path
                .canonicalize()
                .with_context(|| format!("'{}' does not exist", spec))?;
            if !canonical.is_dir() {
                anyhow::bail!("'{}' is not a directory", spec);
            }
            Ok(Source::LocalDir(canonical))
        }
    }

    pub fn fetch(self) -> anyhow::Result<FetchedSource> {
        match self {
            Source::LocalDir(dir) => {
                let name_hint = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("tool")
                    .to_string();
                let version_hint = git_latest_tag(&dir);
                let source_url = format!("file://{}", dir.display());
                Ok(FetchedSource {
                    dir,
                    name_hint,
                    version_hint,
                    source_url,
                    _tempdir: None,
                })
            }
            Source::GitHub {
                owner,
                repo,
                git_ref,
            } => {
                let tmp = tempfile::tempdir().context("failed to create temp dir")?;
                let dest = tmp.path().join("repo");
                let clone_url = format!("https://github.com/{}/{}", owner, repo);

                let mut cmd = std::process::Command::new("git");
                cmd.args(["clone", "--depth", "1"]);
                if let Some(ref r) = git_ref {
                    cmd.args(["--branch", r]);
                }
                cmd.arg(&clone_url).arg(&dest);

                let status = cmd
                    .status()
                    .context("'git clone' failed; is git installed?")?;
                if !status.success() {
                    anyhow::bail!("failed to clone {}", clone_url);
                }

                let version_hint = git_ref
                    .as_deref()
                    .map(|r| r.trim_start_matches('v').to_string())
                    .or_else(|| git_latest_tag(&dest));

                Ok(FetchedSource {
                    dir: dest,
                    name_hint: repo.clone(),
                    version_hint,
                    source_url: clone_url,
                    _tempdir: Some(tmp),
                })
            }
        }
    }
}

fn git_latest_tag(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args([
            "-C",
            &dir.to_string_lossy(),
            "describe",
            "--tags",
            "--abbrev=0",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Some(tag.trim_start_matches('v').to_string())
    } else {
        None
    }
}
