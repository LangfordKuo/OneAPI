use std::{fs, path::Path, sync::atomic::Ordering, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{models::setting, state::AppState};

const OWNER: &str = "LangfordKuo";
const REPO: &str = "OneAPI";
const PROXY_PREFIX: &str = "https://gh-proxy.com/";
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
const HTTP_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
    pub html_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub checksums_url: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateResult {
    pub updated_to: String,
    pub asset_name: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub async fn check(state: &AppState) -> Result<UpdateInfo> {
    let source = configured_source(state).await?;
    let client = client()?;
    let release = fetch_release(&client, &source).await?;

    let current = current_version()?;
    let latest = parse_version(&release.tag_name)?;
    let (asset_name, asset_url, checksums_url) = select_assets(&release, &source)?;

    Ok(UpdateInfo {
        current_version: current.to_string(),
        latest_version: latest.to_string(),
        update_available: latest > current,
        release_notes: release.body,
        published_at: release.published_at,
        html_url: release.html_url,
        asset_name,
        asset_url,
        checksums_url,
        source,
    })
}

pub async fn apply(state: &AppState) -> Result<UpdateResult> {
    if self_update_disabled_in_container() {
        bail!("self-update is disabled inside a container; pull the new image instead");
    }

    if state.updates.in_progress.swap(true, Ordering::SeqCst) {
        bail!("an update is already in progress");
    }

    let result = apply_inner(state).await;
    if result.is_err() {
        state.updates.in_progress.store(false, Ordering::SeqCst);
    }
    result
}

async fn apply_inner(state: &AppState) -> Result<UpdateResult> {
    let info = check(state).await?;
    if !info.update_available {
        bail!("already on the latest version ({})", info.current_version);
    }

    let asset_name = info
        .asset_name
        .clone()
        .ok_or_else(|| anyhow!("this platform is not supported by the latest release"))?;
    let asset_url = info
        .asset_url
        .clone()
        .ok_or_else(|| anyhow!("release asset URL is missing"))?;
    let checksums_url = info
        .checksums_url
        .clone()
        .ok_or_else(|| anyhow!("release does not provide SHA256SUMS; refusing to update"))?;

    let client = client()?;
    let checksums = download(&client, &checksums_url).await?;
    let checksums = String::from_utf8(checksums).context("SHA256SUMS is not valid UTF-8")?;
    let expected = checksum_for(&checksums, &asset_name)
        .ok_or_else(|| anyhow!("SHA256SUMS does not contain {asset_name}"))?;
    let bytes = download(&client, &asset_url).await?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        bail!("checksum mismatch for {asset_name}; refusing to update");
    }

    let temp_dir = state.config.data_dir.join("updates");
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create {}", temp_dir.display()))?;
    let archive = temp_dir.join(&asset_name);
    fs::write(&archive, &bytes)
        .with_context(|| format!("failed to write {}", archive.display()))?;

    let new_binary = temp_dir.join(if cfg!(windows) {
        "oneapi.exe"
    } else {
        "oneapi"
    });
    extract_binary(&archive, &new_binary)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755))
            .context("failed to mark the new binary executable")?;
    }

    let executable = std::env::current_exe().context("failed to locate current executable")?;
    self_replace::self_replace(&new_binary).context("failed to replace the running binary")?;
    if let Ok(mut slot) = state.updates.restart_exe.lock() {
        *slot = Some(executable);
    }
    let _ = fs::remove_file(&archive);
    let _ = fs::remove_file(&new_binary);

    state
        .updates
        .restart_requested
        .store(true, Ordering::SeqCst);
    state.updates.shutdown.notify_one();

    Ok(UpdateResult {
        updated_to: info.latest_version,
        asset_name,
        message: "更新已应用，服务正在重启".to_string(),
    })
}

async fn configured_source(state: &AppState) -> Result<String> {
    let value = setting::get(&state.db, "update_source")
        .await
        .context("failed to read update source setting")?;
    Ok(match value.as_deref() {
        Some("ghproxy") => "ghproxy".to_string(),
        _ => "github".to_string(),
    })
}

fn client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECONDS))
        .user_agent(format!("OneAPI/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build update HTTP client")
}

async fn fetch_release(client: &Client, source: &str) -> Result<GithubRelease> {
    let url = api_url(source);
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("failed to query update metadata from {source}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!(
            "update metadata request failed with {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }

    response
        .json::<GithubRelease>()
        .await
        .context("failed to parse GitHub release metadata")
}

fn api_url(source: &str) -> String {
    if let Ok(base) = std::env::var("ONEAPI_UPDATE_API_BASE") {
        if !base.trim().is_empty() {
            return base;
        }
    }

    let github = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest");
    if source == "ghproxy" {
        format!("{PROXY_PREFIX}{github}")
    } else {
        github
    }
}

fn maybe_proxy(source: &str, url: &str) -> String {
    if source == "ghproxy"
        && (url.starts_with("https://github.com/") || url.starts_with("https://api.github.com/"))
    {
        format!("{PROXY_PREFIX}{url}")
    } else {
        url.to_string()
    }
}

fn select_assets(
    release: &GithubRelease,
    source: &str,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some((platform, extension)) = platform_asset() else {
        return Ok((None, None, None));
    };
    let asset_name = format!("oneapi-{}-{platform}.{extension}", release.tag_name);
    let asset = release.assets.iter().find(|asset| asset.name == asset_name);
    let checksums = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS");

    Ok((
        asset.map(|asset| asset.name.clone()),
        asset.map(|asset| maybe_proxy(source, &asset.browser_download_url)),
        checksums.map(|asset| maybe_proxy(source, &asset.browser_download_url)),
    ))
}

fn platform_asset() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(("linux-amd64", "tar.gz")),
        ("linux", "aarch64") => Some(("linux-arm64", "tar.gz")),
        ("windows", "x86_64") => Some(("windows-amd64", "zip")),
        ("windows", "aarch64") => Some(("windows-arm64", "zip")),
        ("macos", "x86_64") => Some(("macos-amd64", "tar.gz")),
        ("macos", "aarch64") => Some(("macos-arm64", "tar.gz")),
        _ => None,
    }
}

fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("current binary version is not valid semver")
}

fn parse_version(tag: &str) -> Result<Version> {
    Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("release tag `{tag}` is not a valid semver version"))
}

fn checksum_for(checksums: &str, asset_name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset_name).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

async fn download(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;

    if !response.status().is_success() {
        bail!("download failed with {} for {url}", response.status());
    }
    if let Some(length) = response.content_length() {
        if length > MAX_DOWNLOAD_BYTES {
            bail!("download is too large ({length} bytes)");
        }
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read {url}"))?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        bail!("download is too large ({} bytes)", bytes.len());
    }
    Ok(bytes.to_vec())
}

#[cfg(unix)]
fn extract_binary(archive: &Path, output: &Path) -> Result<()> {
    let file =
        fs::File::open(archive).with_context(|| format!("failed to open {}", archive.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().context("failed to read tar archive")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let path = entry.path().context("tar entry has an invalid path")?;
        if path.file_name().and_then(|name| name.to_str()) == Some("oneapi") {
            let mut output = fs::File::create(output)
                .with_context(|| format!("failed to create {}", output.display()))?;
            std::io::copy(&mut entry, &mut output)
                .context("failed to extract the oneapi binary")?;
            return Ok(());
        }
    }

    bail!("archive does not contain the oneapi binary")
}

#[cfg(windows)]
fn extract_binary(archive: &Path, output: &Path) -> Result<()> {
    let file =
        fs::File::open(archive).with_context(|| format!("failed to open {}", archive.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("failed to read zip archive")?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("failed to read zip entry")?;
        if Path::new(entry.name())
            .file_name()
            .and_then(|name| name.to_str())
            == Some("oneapi.exe")
        {
            let mut output = fs::File::create(output)
                .with_context(|| format!("failed to create {}", output.display()))?;
            std::io::copy(&mut entry, &mut output)
                .context("failed to extract the oneapi.exe binary")?;
            return Ok(());
        }
    }

    bail!("archive does not contain the oneapi.exe binary")
}

#[cfg(not(any(unix, windows)))]
fn extract_binary(_archive: &Path, _output: &Path) -> Result<()> {
    bail!("self-update is not supported on this platform")
}

fn self_update_disabled_in_container() -> bool {
    if std::env::var("ONEAPI_ALLOW_SELF_UPDATE")
        .map(|value| value == "1")
        .unwrap_or(false)
    {
        return false;
    }

    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_and_compares() {
        assert!(parse_version("v0.1.1").unwrap() > parse_version("0.1.0").unwrap());
        assert_eq!(parse_version("v0.1.0").unwrap().to_string(), "0.1.0");
    }

    #[test]
    fn finds_checksum_for_asset() {
        let sums = "abc123  oneapi-v0.1.0-linux-amd64.tar.gz\n\
                    def456 *oneapi-v0.1.0-windows-amd64.zip\n";
        assert_eq!(
            checksum_for(sums, "oneapi-v0.1.0-linux-amd64.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            checksum_for(sums, "oneapi-v0.1.0-windows-amd64.zip").as_deref(),
            Some("def456")
        );
        assert_eq!(checksum_for(sums, "missing"), None);
    }

    #[test]
    fn platform_asset_matches_current_target() {
        let (platform, extension) = platform_asset().expect("supported platform");
        assert!(platform.contains("amd64") || platform.contains("arm64"));
        assert!(extension == "tar.gz" || extension == "zip");
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
