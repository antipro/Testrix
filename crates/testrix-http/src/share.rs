use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Supported share channels for publishing a Testrix project's YAML files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareChannel {
    /// Upload YAML bundle to a `testrix-node` Express server.
    TestrixServer,
    /// Upload YAML files directly to Cloudflare R2 storage via the Cloudflare REST API.
    CloudflareR2,
    /// Serve the project's API documentation directly over the local network via an embedded HTTP server.
    LocalNetwork,
}

impl ShareChannel {
    /// Display label for UI selectors.
    pub const fn label(self) -> &'static str {
        match self {
            Self::TestrixServer => "Testrix Share Server",
            Self::CloudflareR2 => "Cloudflare R2 Storage",
            Self::LocalNetwork => "Local Network",
        }
    }

    /// Parses a persisted or display channel label back into a [`ShareChannel`].
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim() {
            "Testrix Share Server" | "Testrix Server" | "TestrixServer" => {
                Some(Self::TestrixServer)
            }
            "Cloudflare R2 Storage" | "Cloudflare R2" | "CloudflareR2" => Some(Self::CloudflareR2),
            "Local Network" | "LocalNetwork" => Some(Self::LocalNetwork),
            _ => None,
        }
    }
}

/// A single YAML file (relative path + UTF-8 contents) collected from a project directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareFile {
    pub path: String,
    pub content: String,
}

/// Result returned after logging into `testrix-node`.
#[derive(Debug, Clone)]
pub struct ShareLoginResult {
    pub email: String,
    pub token: String,
}

/// Result returned after uploading a project to a share channel.
#[derive(Debug, Clone)]
pub struct ShareUploadResult {
    pub channel: ShareChannel,
    pub files_uploaded: usize,
    pub share_url: String,
}

#[derive(Deserialize)]
struct NodeLoginResponse {
    token: Option<String>,
    user: Option<NodeUser>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct NodeUser {
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeUploadRequest<'a> {
    project_slug: &'a str,
    project_name: &'a str,
    access_token: &'a str,
    files: &'a [ShareFile],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeUploadResponse {
    ok: Option<bool>,
    files_written: Option<usize>,
    share_url: Option<String>,
    error: Option<String>,
}

/// Recursively collects all `.yaml` / `.yml` files in `project_dir` as relative paths.
pub fn collect_project_yaml_files(project_dir: &Path) -> Result<Vec<ShareFile>, String> {
    let mut out = Vec::new();
    collect_dir_recursive(project_dir, project_dir, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    if out.is_empty() {
        return Err(format!(
            "No YAML files found in {}",
            project_dir.display()
        ));
    }
    Ok(out)
}

fn collect_dir_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<ShareFile>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|err| format!("Failed reading {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("Failed reading entry: {err}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_dir_recursive(root, &path, out)?;
        } else if path.is_file()
            && (name_str.ends_with(".yaml") || name_str.ends_with(".yml"))
        {
            let rel = path
                .strip_prefix(root)
                .map_err(|err| err.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read_to_string(&path)
                .map_err(|err| format!("Failed reading {}: {err}", path.display()))?;
            out.push(ShareFile { path: rel, content });
        }
    }
    Ok(())
}

/// Runs an async future on a background thread with a current-thread Tokio runtime and returns a oneshot receiver.
pub fn dispatch_share_future<Fut, T>(fut: Fut) -> oneshot::Receiver<T>
where
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            let result = rt.block_on(fut);
            let _ = tx.send(result);
        }
    });
    rx
}

/// Authenticates against `testrix-node` (`POST /api/auth/login`) and returns the bearer token.
pub async fn login_testrix_server(
    server_url: String,
    email: String,
    password: String,
) -> Result<ShareLoginResult, String> {
    let base = server_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Share server URL is required".to_string());
    }
    let endpoint = format!("{base}/api/auth/login");
    let client = reqwest::Client::builder()
        .user_agent("Testrix/0.1")
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let resp = client
        .post(&endpoint)
        .json(&serde_json::json!({
            "email": email.trim(),
            "password": password,
        }))
        .send()
        .await
        .map_err(|err| format!("Failed connecting to {endpoint}: {err}"))?;

    let status = resp.status();
    let parsed: NodeLoginResponse = resp
        .json()
        .await
        .map_err(|err| format!("Invalid response from server ({status}): {err}"))?;

    if !status.is_success() {
        return Err(parsed
            .error
            .unwrap_or_else(|| format!("Login failed with HTTP {status}")));
    }

    let token = parsed
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "Server response did not include a token".to_string())?;
    let user_email = parsed.user.map(|u| u.email).unwrap_or(email);
    Ok(ShareLoginResult {
        email: user_email,
        token,
    })
}

/// Uploads a project's YAML files to `testrix-node` (`POST /api/projects/upload`).
pub async fn upload_to_testrix_server(
    server_url: String,
    token: String,
    project_slug: String,
    project_name: String,
    access_token: String,
    files: Vec<ShareFile>,
) -> Result<ShareUploadResult, String> {
    let base = server_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Share server URL is not configured in Preferences".to_string());
    }
    if token.trim().is_empty() {
        return Err("Not logged in to Testrix Share Server. Please login in Preferences.".to_string());
    }

    let endpoint = format!("{base}/api/projects/upload");
    let client = reqwest::Client::builder()
        .user_agent("Testrix/0.1")
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let payload = NodeUploadRequest {
        project_slug: &project_slug,
        project_name: &project_name,
        access_token: access_token.trim(),
        files: &files,
    };

    let resp = client
        .post(&endpoint)
        .bearer_auth(token.trim())
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("Upload request failed: {err}"))?;

    let status = resp.status();
    let parsed: NodeUploadResponse = resp
        .json()
        .await
        .map_err(|err| format!("Invalid upload response ({status}): {err}"))?;

    if !status.is_success() || parsed.ok != Some(true) {
        return Err(parsed
            .error
            .unwrap_or_else(|| format!("Upload failed with HTTP {status}")));
    }

    let share_url = parsed
        .share_url
        .unwrap_or_else(|| format!("{base}/s/{project_slug}"));
    Ok(ShareUploadResult {
        channel: ShareChannel::TestrixServer,
        files_uploaded: parsed.files_written.unwrap_or(files.len()),
        share_url,
    })
}

#[derive(Deserialize)]
struct CfAccountsResponse {
    result: Option<Vec<CfAccount>>,
}

#[derive(Deserialize)]
struct CfAccount {
    id: String,
}

async fn resolve_cloudflare_account_id(
    client: &reqwest::Client,
    account_id: &str,
    api_token: &str,
) -> Result<String, String> {
    let trimmed = account_id.trim();
    if !trimmed.is_empty() && !trimmed.contains('@') {
        return Ok(trimmed.to_string());
    }

    if let Ok(resp) = client
        .get("https://api.cloudflare.com/client/v4/accounts")
        .bearer_auth(api_token)
        .send()
        .await
        && resp.status().is_success()
        && let Ok(parsed) = resp.json::<CfAccountsResponse>().await
        && let Some(first) = parsed.result.and_then(|v| v.into_iter().next())
        && !first.id.trim().is_empty()
    {
        return Ok(first.id.trim().to_string());
    }

    Err(
        "Cloudflare Account ID must be your 32-character hex Account ID from the Cloudflare Dashboard (not your email address)."
            .to_string(),
    )
}

/// Uploads a project's YAML files directly to Cloudflare R2 using the Cloudflare R2 REST API.
pub async fn upload_to_cloudflare_r2(
    account_id: String,
    bucket: String,
    api_token: String,
    pages_url: String,
    project_slug: String,
    access_token: String,
    files: Vec<ShareFile>,
) -> Result<ShareUploadResult, String> {
    let bucket = bucket.trim();
    let api_token = api_token.trim();
    if bucket.is_empty() || api_token.is_empty() {
        return Err(
            "Cloudflare Account ID, R2 Bucket, and API Token must be configured in Preferences."
                .to_string(),
        );
    }

    let client = reqwest::Client::builder()
        .user_agent("Testrix/0.1")
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let account_id = resolve_cloudflare_account_id(&client, &account_id, api_token).await?;
    let account_id = account_id.as_str();

    let total = files.len();
    for file in files {
        let object_key = format!("{project_slug}/{}", file.path);
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/r2/buckets/{bucket}/objects/{object_key}"
        );
        let resp = client
            .put(&url)
            .bearer_auth(api_token)
            .header("Content-Type", "application/yaml; charset=utf-8")
            .body(file.content)
            .send()
            .await
            .map_err(|err| format!("Failed uploading {object_key} to R2: {err}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "Cloudflare R2 upload failed for {object_key} (HTTP {status}): {text}"
            ));
        }
    }

    // Store optional access token metadata so Cloudflare Pages can enforce private access.
    let access_key = format!("{project_slug}/.testrix-access.json");
    let access_url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/r2/buckets/{bucket}/objects/{access_key}"
    );
    let access_body = serde_json::json!({ "accessToken": access_token.trim() }).to_string();
    let access_resp = client
        .put(&access_url)
        .bearer_auth(api_token)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(access_body)
        .send()
        .await
        .map_err(|err| format!("Failed uploading {access_key} to R2: {err}"))?;
    if !access_resp.status().is_success() {
        let status = access_resp.status();
        let text = access_resp.text().await.unwrap_or_default();
        return Err(format!(
            "Cloudflare R2 upload failed for {access_key} (HTTP {status}): {text}"
        ));
    }

    let pages_base = pages_url.trim().trim_end_matches('/');
    let share_url = if pages_base.is_empty() {
        format!("r2://{bucket}/{project_slug}/")
    } else {
        format!("{pages_base}/?project={project_slug}")
    };

    Ok(ShareUploadResult {
        channel: ShareChannel::CloudflareR2,
        files_uploaded: total,
        share_url,
    })
}

/// Revokes (deletes) a shared project on `testrix-node` (`DELETE /api/my/projects/by-slug/:slug`).
pub async fn revoke_testrix_server_share(
    server_url: String,
    token: String,
    project_slug: String,
) -> Result<(), String> {
    let base = server_url.trim().trim_end_matches('/');
    if base.is_empty() || token.trim().is_empty() {
        return Err("Share server URL and login token are required to revoke".to_string());
    }
    let endpoint = format!("{base}/api/my/projects/by-slug/{project_slug}");
    let client = reqwest::Client::builder()
        .user_agent("Testrix/0.1")
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let resp = client
        .delete(&endpoint)
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|err| format!("Failed revoking on server: {err}"))?;

    if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NOT_FOUND {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Server revoke failed (HTTP {status}): {text}"));
    }
    Ok(())
}

/// Revokes (deletes) a project's uploaded YAML objects from Cloudflare R2.
pub async fn revoke_cloudflare_r2_share(
    account_id: String,
    bucket: String,
    api_token: String,
    project_slug: String,
    file_paths: Vec<String>,
) -> Result<(), String> {
    let bucket = bucket.trim();
    let api_token = api_token.trim();
    if bucket.is_empty() || api_token.is_empty() {
        return Err("Cloudflare R2 credentials are required to revoke".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("Testrix/0.1")
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let account_id = resolve_cloudflare_account_id(&client, &account_id, api_token).await?;
    let account_id = account_id.as_str();

    let mut keys: Vec<String> = file_paths
        .into_iter()
        .map(|p| format!("{project_slug}/{p}"))
        .collect();
    keys.push(format!("{project_slug}/.testrix-access.json"));

    for key in keys {
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/r2/buckets/{bucket}/objects/{key}"
        );
        let resp = client
            .delete(&url)
            .bearer_auth(api_token)
            .send()
            .await
            .map_err(|err| format!("Failed deleting {key} from R2: {err}"))?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NOT_FOUND {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("R2 delete failed for {key} (HTTP {status}): {text}"));
        }
    }
    Ok(())
}
