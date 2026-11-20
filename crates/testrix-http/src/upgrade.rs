//! GitHub Releases upgrade checking, asset downloading, and installer launching.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use serde::Deserialize;
use tokio::io::AsyncWriteExt;

/// GitHub REST API endpoint for the latest Testrix release.
pub const GITHUB_RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/antipro/Testrix/releases/latest";

/// Public GitHub Releases page used as a fallback link.
pub const GITHUB_RELEASES_PAGE: &str = "https://github.com/antipro/Testrix/releases";

/// Metadata for a single downloadable release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

/// Summary of the latest GitHub release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub html_url: String,
    pub asset: Option<ReleaseAsset>,
}

/// Result of comparing the running version against the latest GitHub release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeCheckResult {
    pub current_version: String,
    pub latest: ReleaseInfo,
    pub has_upgrade: bool,
}

/// Thread-safe download progress tracker shared between the downloader and UI.
#[derive(Debug, Default)]
pub struct DownloadProgressState {
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
}

impl DownloadProgressState {
    /// Creates a new zeroed progress tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `(downloaded_bytes, total_bytes_if_known)`.
    pub fn snapshot(&self) -> (u64, Option<u64>) {
        let downloaded = self.downloaded.load(AtomicOrdering::Relaxed);
        let total = self.total.load(AtomicOrdering::Relaxed);
        (downloaded, (total > 0).then_some(total))
    }
}

#[derive(Debug, Deserialize)]
struct GitHubReleasePayload {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAssetPayload>,
}

#[derive(Debug, Deserialize)]
struct GitHubAssetPayload {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<String>,
}

fn parse_version(raw: &str) -> Option<ParsedVersion> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if stripped.is_empty() {
        return None;
    }
    let (core, prerelease) = match stripped.split_once('-') {
        Some((c, pre)) => (c, Some(pre.trim().to_string()).filter(|s| !s.is_empty())),
        None => (stripped, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts
        .next()
        .map(|s| s.parse::<u64>())
        .transpose()
        .ok()?
        .unwrap_or(0);
    let patch = parts
        .next()
        .map(|s| s.parse::<u64>())
        .transpose()
        .ok()?
        .unwrap_or(0);

    Some(ParsedVersion {
        major,
        minor,
        patch,
        prerelease,
    })
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ap), Some(bp)) => {
                let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
                    (Ok(an), Ok(bn)) => an.cmp(&bn),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => ap.cmp(bp),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Returns `true` when `latest` represents a strictly newer version than `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let Some(cur) = parse_version(current) else {
        return false;
    };
    let Some(lat) = parse_version(latest) else {
        return false;
    };

    match (lat.major, lat.minor, lat.patch).cmp(&(cur.major, cur.minor, cur.patch)) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => match (cur.prerelease.as_deref(), lat.prerelease.as_deref()) {
            (Some(_), None) => true,
            (None, Some(_)) | (None, None) => false,
            (Some(cp), Some(lp)) => compare_prerelease(lp, cp) == Ordering::Greater,
        },
    }
}

fn linux_prefers_deb() -> bool {
    Path::new("/etc/debian_version").exists() || Path::new("/usr/bin/dpkg").exists()
}

/// Selects the most appropriate installer asset for the current operating system.
pub fn select_platform_asset(assets: &[ReleaseAsset]) -> Option<ReleaseAsset> {
    select_asset_for_os(assets, std::env::consts::OS, linux_prefers_deb())
}

fn select_asset_for_os(
    assets: &[ReleaseAsset],
    target_os: &str,
    prefer_deb: bool,
) -> Option<ReleaseAsset> {
    let find_ext = |ext: &str| -> Option<ReleaseAsset> {
        assets
            .iter()
            .find(|a| a.name.to_ascii_lowercase().ends_with(ext))
            .cloned()
    };

    match target_os {
        "macos" => find_ext(".dmg"),
        "windows" => find_ext(".msi").or_else(|| find_ext(".exe")),
        "linux" => {
            if prefer_deb {
                find_ext(".deb").or_else(|| find_ext(".rpm"))
            } else {
                find_ext(".rpm").or_else(|| find_ext(".deb"))
            }
        }
        _ => None,
    }
}

fn parse_release_payload(payload: GitHubReleasePayload) -> Option<ReleaseInfo> {
    if payload.draft || payload.prerelease {
        return None;
    }
    let assets: Vec<ReleaseAsset> = payload
        .assets
        .into_iter()
        .map(|a| ReleaseAsset {
            name: a.name,
            download_url: a.browser_download_url,
            size: a.size,
        })
        .collect();
    let asset = select_platform_asset(&assets);
    let html_url = if payload.html_url.trim().is_empty() {
        GITHUB_RELEASES_PAGE.to_string()
    } else {
        payload.html_url
    };
    let name = if payload.name.trim().is_empty() {
        payload.tag_name.clone()
    } else {
        payload.name
    };
    Some(ReleaseInfo {
        tag_name: payload.tag_name,
        name,
        body: payload.body,
        html_url,
        asset,
    })
}

/// Checks GitHub Releases for a newer version of Testrix.
pub async fn check_for_upgrade(current_version: &str) -> Result<UpgradeCheckResult, String> {
    check_for_upgrade_from_url(GITHUB_RELEASES_LATEST_API, current_version).await
}

/// Checks a specific GitHub Releases JSON endpoint for a newer version.
pub async fn check_for_upgrade_from_url(
    api_url: &str,
    current_version: &str,
) -> Result<UpgradeCheckResult, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("Testrix/{current_version}"))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| format!("Failed creating HTTP client: {err}"))?;

    let resp = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| format!("Failed checking for updates: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub API returned {status}: {body}"));
    }

    let payload: GitHubReleasePayload = resp
        .json()
        .await
        .map_err(|err| format!("Invalid GitHub release payload: {err}"))?;

    let release = parse_release_payload(payload)
        .ok_or_else(|| "Latest GitHub release is a draft or prerelease".to_string())?;
    let has_upgrade = is_newer_version(current_version, &release.tag_name);

    Ok(UpgradeCheckResult {
        current_version: current_version.to_string(),
        latest: release,
        has_upgrade,
    })
}

/// Downloads a release asset into a temporary updates directory while updating `progress`.
pub async fn download_release_asset(
    url: &str,
    file_name: &str,
    progress: Arc<DownloadProgressState>,
) -> Result<PathBuf, String> {
    let safe_name = Path::new(file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "Invalid release asset filename".to_string())?;

    let updates_dir = std::env::temp_dir().join("testrix-updates");
    tokio::fs::create_dir_all(&updates_dir)
        .await
        .map_err(|err| format!("Failed creating updates directory: {err}"))?;

    let dest_path = updates_dir.join(safe_name);
    let part_path = updates_dir.join(format!("{safe_name}.part"));

    let client = reqwest::Client::builder()
        .user_agent("Testrix-Updater")
        .build()
        .map_err(|err| format!("Failed creating download client: {err}"))?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("Failed downloading update: {err}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed with HTTP {}", resp.status()));
    }

    if let Some(len) = resp.content_length() {
        progress.total.store(len, AtomicOrdering::Relaxed);
    }
    progress.downloaded.store(0, AtomicOrdering::Relaxed);

    let mut file = tokio::fs::File::create(&part_path)
        .await
        .map_err(|err| format!("Failed creating installer file: {err}"))?;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|err| format!("Failed reading download stream: {err}"))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|err| format!("Failed writing installer file: {err}"))?;
        progress
            .downloaded
            .fetch_add(chunk.len() as u64, AtomicOrdering::Relaxed);
    }

    file.flush()
        .await
        .map_err(|err| format!("Failed flushing installer file: {err}"))?;
    drop(file);

    if dest_path.exists() {
        let _ = tokio::fs::remove_file(&dest_path).await;
    }
    tokio::fs::rename(&part_path, &dest_path)
        .await
        .map_err(|err| format!("Failed finalizing installer file: {err}"))?;

    Ok(dest_path)
}

/// Launches the downloaded installer using the host OS's native mechanism.
pub fn launch_installer(installer_path: &Path) -> Result<(), String> {
    if !installer_path.exists() {
        return Err(format!(
            "Installer file does not exist: {}",
            installer_path.display()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(installer_path)
            .spawn()
            .map_err(|err| format!("Failed opening DMG installer: {err}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let is_msi = installer_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("msi"));
        if is_msi {
            std::process::Command::new("msiexec")
                .arg("/i")
                .arg(installer_path)
                .spawn()
                .map_err(|err| format!("Failed starting MSI installer: {err}"))?;
        } else {
            std::process::Command::new(installer_path)
                .spawn()
                .map_err(|err| format!("Failed starting installer executable: {err}"))?;
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(installer_path)
            .spawn()
            .map_err(|err| format!("Failed opening package installer: {err}"))?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = installer_path;
        Err("Automatic installer launch is not supported on this platform".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_including_v_prefix_and_prerelease() {
        assert!(is_newer_version("v0.1.0", "v0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("v0.1.0-rc.1", "v0.1.0"));
        assert!(is_newer_version("v0.1.0-beta.1", "v0.1.0-beta.2"));
        assert!(!is_newer_version("v0.2.0", "v0.2.0"));
        assert!(!is_newer_version("v0.2.0", "v0.1.9"));
        assert!(!is_newer_version("v0.2.0", "v0.2.0-rc.1"));
    }

    #[test]
    fn selects_expected_asset_per_os() {
        let assets = vec![
            ReleaseAsset {
                name: "Testrix-0.2.0-macos-universal.dmg".to_string(),
                download_url: "https://example.com/mac.dmg".to_string(),
                size: 100,
            },
            ReleaseAsset {
                name: "Testrix-0.2.0-windows-x86_64.msi".to_string(),
                download_url: "https://example.com/win.msi".to_string(),
                size: 200,
            },
            ReleaseAsset {
                name: "Testrix-0.2.0-windows-x86_64.exe".to_string(),
                download_url: "https://example.com/win.exe".to_string(),
                size: 150,
            },
            ReleaseAsset {
                name: "Testrix-0.2.0-linux-amd64.deb".to_string(),
                download_url: "https://example.com/linux.deb".to_string(),
                size: 120,
            },
            ReleaseAsset {
                name: "Testrix-0.2.0-linux-x86_64.rpm".to_string(),
                download_url: "https://example.com/linux.rpm".to_string(),
                size: 130,
            },
        ];

        assert_eq!(
            select_asset_for_os(&assets, "macos", true)
                .unwrap()
                .name,
            "Testrix-0.2.0-macos-universal.dmg"
        );
        assert_eq!(
            select_asset_for_os(&assets, "windows", true)
                .unwrap()
                .name,
            "Testrix-0.2.0-windows-x86_64.msi"
        );
        assert_eq!(
            select_asset_for_os(&assets, "linux", true)
                .unwrap()
                .name,
            "Testrix-0.2.0-linux-amd64.deb"
        );
        assert_eq!(
            select_asset_for_os(&assets, "linux", false)
                .unwrap()
                .name,
            "Testrix-0.2.0-linux-x86_64.rpm"
        );
    }
}
