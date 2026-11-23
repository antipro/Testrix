use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Result of authenticating with a `node-web` server for Git Sync.
#[derive(Debug, Clone)]
pub struct SyncLoginResult {
    pub token: String,
    pub email: String,
}

/// Summary of a remote Git Sync repository on `node-web`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncRepoSummary {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub recent_commits: usize,
    #[serde(default)]
    pub updated_at: String,
}

/// Full file tree and HEAD commit OID fetched from a `node-web` Git Sync repository.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncState {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// Result of pushing a merged tree to `node-web`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncPushResult {
    pub slug: String,
    #[serde(default)]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub committed: bool,
    #[serde(default)]
    pub files_count: usize,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginUserBody {
    email: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponseBody {
    token: Option<String>,
    user: Option<LoginUserBody>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListReposResponse {
    repos: Vec<RemoteSyncRepoSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PushSyncBody<'a> {
    expected_head_oid: Option<&'a str>,
    files: &'a BTreeMap<String, String>,
    replace_tree: bool,
    message: &'a str,
    name: &'a str,
}

fn normalize_base_url(server_url: &str) -> Result<String, String> {
    let base = server_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("Sync server URL is required.".to_string());
    }
    Ok(base)
}

async fn extract_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if let Ok(parsed) = serde_json::from_str::<ApiErrorBody>(&text)
        && let Some(err) = parsed.error
    {
        return err;
    }
    if text.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {}", text.trim())
    }
}

/// Signs in to `node-web` (`POST /api/auth/login`) and returns a bearer token for Git Sync.
pub async fn login_sync_server(
    server_url: &str,
    email: &str,
    password: &str,
) -> Result<SyncLoginResult, String> {
    let base = normalize_base_url(server_url)?;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({
            "email": email.trim(),
            "password": password,
        }))
        .send()
        .await
        .map_err(|e| format!("Failed connecting to sync server: {e}"))?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    let parsed: LoginResponseBody = serde_json::from_str(&body_text)
        .map_err(|e| format!("Invalid login response ({status}): {e}"))?;

    if !status.is_success() {
        return Err(parsed
            .error
            .unwrap_or_else(|| format!("Login failed with HTTP {status}")));
    }

    let token = parsed
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "Server did not return an auth token".to_string())?;
    let user_email = parsed
        .user
        .map(|u| u.email)
        .unwrap_or_else(|| email.trim().to_string());

    Ok(SyncLoginResult {
        token,
        email: user_email,
    })
}

/// Lists Git Sync repositories owned by the authenticated user on `node-web`.
pub async fn list_remote_sync_repos(
    server_url: &str,
    token: &str,
) -> Result<Vec<RemoteSyncRepoSummary>, String> {
    let base = normalize_base_url(server_url)?;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/sync/repos"))
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|e| format!("Failed fetching sync repos: {e}"))?;

    if !resp.status().is_success() {
        return Err(extract_error(resp).await);
    }

    let parsed: ListReposResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid sync repos response: {e}"))?;
    Ok(parsed.repos)
}

/// Fetches the current file tree and HEAD commit OID for `repo_slug` on `node-web`
/// (creating the remote repo if it does not exist yet).
pub async fn fetch_remote_sync_state(
    server_url: &str,
    token: &str,
    repo_slug: &str,
) -> Result<RemoteSyncState, String> {
    let base = normalize_base_url(server_url)?;
    let slug = repo_slug.trim();
    if slug.is_empty() {
        return Err("Sync repository slug is required.".to_string());
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/sync/repos/{slug}/state?create=1"))
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|e| format!("Failed fetching remote sync state: {e}"))?;

    if !resp.status().is_success() {
        return Err(extract_error(resp).await);
    }

    resp.json::<RemoteSyncState>()
        .await
        .map_err(|e| format!("Invalid remote sync state JSON: {e}"))
}

/// Pushes a merged file tree to `node-web` (`POST /api/sync/repos/:slug/push`) with optimistic
/// concurrency checking against `expected_head_oid`.
pub async fn push_merged_sync_state(
    server_url: &str,
    token: &str,
    repo_slug: &str,
    expected_head_oid: Option<&str>,
    files: &BTreeMap<String, String>,
    message: &str,
) -> Result<RemoteSyncPushResult, String> {
    let base = normalize_base_url(server_url)?;
    let slug = repo_slug.trim();
    if slug.is_empty() {
        return Err("Sync repository slug is required.".to_string());
    }
    let client = reqwest::Client::new();
    let body = PushSyncBody {
        expected_head_oid,
        files,
        replace_tree: true,
        message,
        name: slug,
    };

    let resp = client
        .post(format!("{base}/api/sync/repos/{slug}/push"))
        .bearer_auth(token.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed pushing sync state: {e}"))?;

    if !resp.status().is_success() {
        return Err(extract_error(resp).await);
    }

    resp.json::<RemoteSyncPushResult>()
        .await
        .map_err(|e| format!("Invalid sync push response JSON: {e}"))
}
