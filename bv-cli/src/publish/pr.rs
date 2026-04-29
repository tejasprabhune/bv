use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use owo_colors::{OwoColorize, Stream};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::{Value, json};

const GH_API: &str = "https://api.github.com";
const BV_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Error from a GitHub API call that preserves the HTTP status and parsed
/// body, so callers can react to specific failure modes (e.g. 422 with
/// "Reference already exists") instead of relying on substring matching.
#[derive(Debug)]
struct GhError {
    status: StatusCode,
    body: Value,
    method: &'static str,
    url: String,
}

impl std::fmt::Display for GhError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} -> {}: {}", self.method, self.url, self.status, self.body)
    }
}

impl std::error::Error for GhError {}

/// True iff the JSON body is a GitHub 422 indicating the git ref already
/// exists. GitHub returns either `{"message":"Reference already exists"}` or
/// `{"errors":[{"message":"... Reference already exists ..."}]}`.
fn is_reference_already_exists(body: &Value) -> bool {
    if body
        .get("message")
        .and_then(|m| m.as_str())
        .is_some_and(|m| m.contains("Reference already exists"))
    {
        return true;
    }
    if let Some(errs) = body.get("errors").and_then(|e| e.as_array()) {
        for err in errs {
            if err
                .get("message")
                .and_then(|m| m.as_str())
                .is_some_and(|m| m.contains("Reference already exists"))
            {
                return true;
            }
        }
    }
    false
}

/// Result of a typed POST: either a transport-level failure (network etc.)
/// or a non-success HTTP response that callers may want to inspect.
enum PostError {
    Transport(anyhow::Error),
    Status(GhError),
}

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
        self.post_typed(url, body).await.map_err(|e| match e {
            PostError::Transport(err) => err,
            PostError::Status(gh) => anyhow::Error::new(gh),
        })
    }

    async fn post_typed(&self, url: &str, body: Value) -> Result<Value, PostError> {
        let resp = self
            .client
            .post(url)
            .header(AUTHORIZATION, self.auth_header())
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, format!("bv-cli/{}", BV_VERSION))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                PostError::Transport(anyhow::Error::new(e).context(format!("POST {}", url)))
            })?;

        let status = resp.status();
        let resp_body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() && status.as_u16() != 202 {
            return Err(PostError::Status(GhError {
                status,
                body: resp_body,
                method: "POST",
                url: url.to_string(),
            }));
        }
        Ok(resp_body)
    }

    /// Like `get`, but returns `Ok(None)` on 404 instead of bubbling an error.
    async fn get_opt(&self, url: &str) -> anyhow::Result<Option<Value>> {
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
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("GET {} -> {}: {}", url, status, body);
        }
        Ok(Some(body))
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

    // Create branch on the fork. A 422 with "Reference already exists" means
    // the branch is already there from a previous run; treat that as a
    // benign overlap and continue (we'll update the file in place below).
    // Any other 422 (or other failure) is a real error.
    let create_ref_url = format!("{}/repos/{}/git/refs", GH_API, fork_full_name);
    let branch_result = gh
        .post_typed(
            &create_ref_url,
            json!({
                "ref": format!("refs/heads/{}", branch_name),
                "sha": fork_sha
            }),
        )
        .await;

    match branch_result {
        Ok(_) => {}
        Err(PostError::Transport(e)) => return Err(e),
        Err(PostError::Status(gh_err)) => {
            if gh_err.status == StatusCode::UNPROCESSABLE_ENTITY
                && is_reference_already_exists(&gh_err.body)
            {
                eprintln!(
                    "  {} branch '{}' already exists on fork; updating in place",
                    "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
                    branch_name
                );
            } else {
                return Err(anyhow::Error::new(gh_err));
            }
        }
    }

    // Upload manifest file. If the file already exists on this branch (rerun
    // case), GitHub's PUT requires the existing blob's `sha` in the body to
    // turn the create into an update. Without it, GitHub returns 422 with no
    // friendly hint.
    let file_path = format!("tools/{}/{}.toml", ctx.tool_name, ctx.version);
    let content_b64 = STANDARD.encode(ctx.manifest_toml);
    let commit_msg = format!(
        "Add {name} {version}",
        name = ctx.tool_name,
        version = ctx.version
    );

    let contents_url = format!("{}/repos/{}/contents/{}", GH_API, fork_full_name, file_path);
    let existing_sha = gh
        .get_opt(&format!("{}?ref={}", contents_url, branch_name))
        .await?
        .and_then(|v| v.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string()));

    let mut put_body = json!({
        "message": commit_msg,
        "content": content_b64,
        "branch": branch_name
    });
    if let Some(sha) = existing_sha {
        put_body["sha"] = json!(sha);
    }

    gh.put(&contents_url, put_body).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_reference_already_exists_top_level() {
        let body = json!({"message": "Reference already exists", "documentation_url": "..."});
        assert!(is_reference_already_exists(&body));
    }

    #[test]
    fn detects_reference_already_exists_nested() {
        let body = json!({
            "message": "Validation Failed",
            "errors": [{"resource": "Reference", "code": "custom",
                        "message": "Reference already exists"}]
        });
        assert!(is_reference_already_exists(&body));
    }

    #[test]
    fn does_not_misclassify_other_422s() {
        let body = json!({
            "message": "Validation Failed",
            "errors": [{"message": "sha wasn't supplied"}]
        });
        assert!(!is_reference_already_exists(&body));
    }

    #[test]
    fn does_not_misclassify_empty_body() {
        assert!(!is_reference_already_exists(&Value::Null));
        assert!(!is_reference_already_exists(&json!({})));
    }
}
