use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use owo_colors::{OwoColorize, Stream};
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::{Value, json};

const GH_API: &str = "https://api.github.com";
const BV_VERSION: &str = env!("CARGO_PKG_VERSION");

struct GhClient {
    client: reqwest::Client,
    token: String,
}

impl GhClient {
    fn new(token: &str) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("failed to build HTTP client")?;
        Ok(GhClient {
            client,
            token: token.to_string(),
        })
    }

    fn auth_header(&self) -> String {
        format!("token {}", self.token)
    }

    async fn get(&self, url: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, self.auth_header())
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, format!("bv-cli/{}", BV_VERSION))
            .send()
            .await
            .with_context(|| format!("GET {}", url))?;

        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("GET {} -> {}: {}", url, status, body);
        }
        Ok(body)
    }

    async fn post(&self, url: &str, body: Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .post(url)
            .header(AUTHORIZATION, self.auth_header())
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, format!("bv-cli/{}", BV_VERSION))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {}", url))?;

        let status = resp.status();
        let resp_body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() && status.as_u16() != 202 {
            anyhow::bail!("POST {} -> {}: {}", url, status, resp_body);
        }
        Ok(resp_body)
    }

    async fn put(&self, url: &str, body: Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .put(url)
            .header(AUTHORIZATION, self.auth_header())
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, format!("bv-cli/{}", BV_VERSION))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("PUT {}", url))?;

        let status = resp.status();
        let resp_body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("PUT {} -> {}: {}", url, status, resp_body);
        }
        Ok(resp_body)
    }
}

pub struct PrContext<'a> {
    pub tool_name: &'a str,
    pub version: &'a str,
    pub manifest_toml: &'a str,
    pub github_token: &'a str,
    pub registry_repo: &'a str,
    pub source_url: &'a str,
}

pub async fn open_pr(ctx: PrContext<'_>) -> anyhow::Result<String> {
    let gh = GhClient::new(ctx.github_token)?;

    let (_upstream_owner, upstream_repo_name) = ctx
        .registry_repo
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("registry_repo must be 'owner/repo'"))?;

    eprintln!(
        "  {} {} ...",
        "Preparing PR to".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        ctx.registry_repo
    );

    // Get authenticated user's login.
    let user_info = gh.get(&format!("{}/user", GH_API)).await?;
    let username = user_info["login"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("could not get GitHub username from /user"))?
        .to_string();

    // Get upstream repo info (default branch).
    let repo_info = gh
        .get(&format!("{}/repos/{}", GH_API, ctx.registry_repo))
        .await?;
    let default_branch = repo_info["default_branch"]
        .as_str()
        .unwrap_or("main")
        .to_string();

    // Fork the registry repo.
    let fork_info = gh
        .post(
            &format!("{}/repos/{}/forks", GH_API, ctx.registry_repo),
            json!({}),
        )
        .await?;

    let fork_full_name = fork_info["full_name"]
        .as_str()
        .unwrap_or(&format!("{}/{}", username, upstream_repo_name))
        .to_string();

    // Wait for fork to become ready (GitHub may take a few seconds).
    let fork_sha = wait_for_fork_branch(&gh, &fork_full_name, &default_branch).await?;

    let branch_name = format!("publish/{}/{}", ctx.tool_name, ctx.version);

    // Create branch on the fork.
    let create_ref_url = format!("{}/repos/{}/git/refs", GH_API, fork_full_name);
    let branch_result = gh
        .post(
            &create_ref_url,
            json!({
                "ref": format!("refs/heads/{}", branch_name),
                "sha": fork_sha
            }),
        )
        .await;

    if let Err(e) = branch_result {
        if e.to_string().contains("422") || e.to_string().contains("already exists") {
            anyhow::bail!(
                "branch '{}' already exists on fork\n  \
                 Delete it on GitHub and retry, or check for an existing PR",
                branch_name
            );
        }
        return Err(e);
    }

    // Upload manifest file.
    let file_path = format!("tools/{}/{}.toml", ctx.tool_name, ctx.version);
    let content_b64 = STANDARD.encode(ctx.manifest_toml);
    let commit_msg = format!(
        "Add {name} {version}",
        name = ctx.tool_name,
        version = ctx.version
    );

    gh.put(
        &format!("{}/repos/{}/contents/{}", GH_API, fork_full_name, file_path),
        json!({
            "message": commit_msg,
            "content": content_b64,
            "branch": branch_name
        }),
    )
    .await?;

    eprintln!(
        "    {} tools/{}/{}.toml",
        "Committed".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        ctx.tool_name,
        ctx.version
    );

    // Open PR from fork to upstream.
    let pr_title = format!(
        "Add {name} {version}",
        name = ctx.tool_name,
        version = ctx.version
    );
    let pr_body = pr_body(ctx.tool_name, ctx.version, ctx.source_url);

    let pr_info = gh
        .post(
            &format!("{}/repos/{}/pulls", GH_API, ctx.registry_repo),
            json!({
                "title": pr_title,
                "body": pr_body,
                "head": format!("{}:{}", username, branch_name),
                "base": default_branch
            }),
        )
        .await?;

    let pr_url = pr_info["html_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("PR URL not found in response"))?
        .to_string();

    Ok(pr_url)
}

async fn wait_for_fork_branch(
    gh: &GhClient,
    fork_full_name: &str,
    branch: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/repos/{}/git/refs/heads/{}",
        GH_API, fork_full_name, branch
    );

    for attempt in 0..12 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            eprint!(".");
        }
        match gh.get(&url).await {
            Ok(body) => {
                if attempt > 0 {
                    eprintln!();
                }
                let sha = body["object"]["sha"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("could not read fork branch SHA"))?
                    .to_string();
                return Ok(sha);
            }
            Err(_) => {
                if attempt == 0 {
                    eprint!(
                        "  {} fork",
                        "Waiting for".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
                    );
                }
            }
        }
    }
    anyhow::bail!(
        "fork '{}' did not become ready in time; try again in a minute",
        fork_full_name
    )
}

fn pr_body(name: &str, version: &str, source_url: &str) -> String {
    format!(
        "## Add {name} {version}\n\n\
         Published via `bv publish`.\n\n\
         **Source:** {source_url}\n\n\
         ### Checklist\n\
         - [ ] Typed I/O is declared\n\
         - [ ] Image pulls and runs correctly\n\
         - [ ] Entrypoint command is correct\n"
    )
}

/// Get the GitHub username for the given token (needed for docker login).
pub async fn get_github_username(token: &str) -> anyhow::Result<String> {
    let gh = GhClient::new(token)?;
    let info = gh.get(&format!("{}/user", GH_API)).await?;
    info["login"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("could not read username from GitHub API"))
}
