use std::{fs, path::Path, sync::atomic::Ordering, time::Duration};

use anyhow::{bail, Context, Result};
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

/// What failed during an update. The kind decides how loudly the UI reports it:
/// transport problems are usually fixed by switching the update source, while
/// integrity and replacement failures need manual inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorKind {
    /// Metadata or download could not be fetched from the update source.
    Network,
    /// The downloaded package or its checksum cannot be trusted.
    Integrity,
    /// This platform has no installable asset.
    Platform,
    /// The new binary could not be installed over the running one.
    Replace,
    /// The update directory or the database could not be used.
    Storage,
    /// The request cannot run now (already running, already current, container).
    State,
}

impl UpdateErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateErrorKind::Network => "network",
            UpdateErrorKind::Integrity => "integrity",
            UpdateErrorKind::Platform => "platform",
            UpdateErrorKind::Replace => "replace",
            UpdateErrorKind::Storage => "storage",
            UpdateErrorKind::State => "state",
        }
    }
}

/// A classified update failure. `hint` is the troubleshooting step shown to the
/// administrator, so it names what to check instead of restating the error.
#[derive(Debug)]
pub struct UpdateError {
    pub kind: UpdateErrorKind,
    pub message: String,
    pub hint: &'static str,
}

impl UpdateError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Network,
            message: message.into(),
            hint: "可在「设置 → 更新源」切换为国内加速后重试；若仍然失败，请检查服务器能否访问 GitHub 或代理地址。",
        }
    }

    fn integrity(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Integrity,
            message: message.into(),
            hint:
                "请切换更新源后重试；若仍失败，请手动下载发布包并核对 SHA256SUMS 后再替换二进制。",
        }
    }

    fn platform(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Platform,
            message: message.into(),
            hint: "当前平台没有可用的发布包，请改用 Docker 镜像或手动下载对应平台的可执行文件。",
        }
    }

    fn replace(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Replace,
            message: message.into(),
            hint: "请确认程序所在目录可写、磁盘空间充足、没有其他进程占用该二进制；容器部署请改为拉取新镜像。",
        }
    }

    fn storage(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Storage,
            message: message.into(),
            hint: "请检查数据目录下 updates/ 的写入权限与磁盘剩余空间后重试。",
        }
    }

    fn database(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Storage,
            message: message.into(),
            hint: "读取更新设置失败，请检查数据库是否可读写（见仪表盘「数据库」状态）；必要时从备份恢复后重试。",
        }
    }

    fn state(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::State,
            message: message.into(),
            hint: "请稍后重试；也可以在发布页手动下载安装包完成更新。",
        }
    }

    /// `warning` is recoverable by retrying or switching the update source;
    /// `error` means the install itself failed and needs manual checks.
    pub fn severity(&self) -> &'static str {
        match self.kind {
            UpdateErrorKind::Network | UpdateErrorKind::Platform | UpdateErrorKind::State => {
                "warning"
            }
            UpdateErrorKind::Integrity | UpdateErrorKind::Replace | UpdateErrorKind::Storage => {
                "error"
            }
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UpdateError {}

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
    let source = configured_source(state)
        .await
        .map_err(|error| UpdateError::database(error.to_string()))?;
    let client = client()?;
    let release = fetch_release(&client, &source)
        .await
        .map_err(|error| UpdateError::network(error.to_string()))?;

    let current = current_version()?;
    let latest = parse_version(&release.tag_name)
        .map_err(|error| UpdateError::network(error.to_string()))?;
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
        return Err(UpdateError::state("容器内不支持自动更新，请拉取新镜像后重建容器").into());
    }

    if state.updates.in_progress.swap(true, Ordering::SeqCst) {
        return Err(UpdateError::state("已有更新正在进行，请稍后再试").into());
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
        return Err(
            UpdateError::state(format!("当前已是最新版本（{}）", info.current_version)).into(),
        );
    }

    let asset_name = info
        .asset_name
        .clone()
        .ok_or_else(|| UpdateError::platform("最新版本没有提供当前平台的安装包"))?;
    validate_asset_name(&asset_name).map_err(|error| UpdateError::integrity(error.to_string()))?;
    let asset_url = info
        .asset_url
        .clone()
        .ok_or_else(|| UpdateError::integrity("发布信息缺少安装包下载地址"))?;
    let checksums_url = info
        .checksums_url
        .clone()
        .ok_or_else(|| UpdateError::integrity("发布页缺少 SHA256SUMS，已拒绝更新"))?;

    let client = client()?;
    let checksums = download(&client, &checksums_url)
        .await
        .map_err(|error| UpdateError::network(error.to_string()))?;
    let checksums = String::from_utf8(checksums)
        .map_err(|_| UpdateError::integrity("SHA256SUMS 不是合法的 UTF-8 文本"))?;
    let expected = checksum_for(&checksums, &asset_name)
        .ok_or_else(|| UpdateError::integrity(format!("SHA256SUMS 中找不到 {asset_name}")))?;
    let bytes = download(&client, &asset_url)
        .await
        .map_err(|error| UpdateError::network(error.to_string()))?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(
            UpdateError::integrity(format!("{asset_name} 的校验和不匹配，已拒绝更新")).into(),
        );
    }

    let temp_dir = state.config.data_dir.join("updates");
    fs::create_dir_all(&temp_dir).map_err(|error| {
        UpdateError::storage(format!("无法创建更新目录 {}：{error}", temp_dir.display()))
    })?;
    let archive = temp_dir.join(&asset_name);
    fs::write(&archive, &bytes).map_err(|error| {
        UpdateError::storage(format!("无法写入 {}：{error}", archive.display()))
    })?;

    let new_binary = temp_dir.join(if cfg!(windows) {
        "oneapi.exe"
    } else {
        "oneapi"
    });
    extract_binary(&archive, &new_binary)
        .map_err(|error| UpdateError::integrity(format!("安装包解压失败：{error}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755))
            .map_err(|error| UpdateError::replace(format!("无法设置新二进制权限：{error}")))?;
    }

    let executable = std::env::current_exe()
        .map_err(|error| UpdateError::replace(format!("无法定位当前可执行文件：{error}")))?;
    self_replace::self_replace(&new_binary)
        .map_err(|error| UpdateError::replace(format!("替换运行中的二进制失败：{error}")))?;
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
    // Metrics SSE streams only listen to the watch channel; notify_one would
    // leave them open and block the graceful shutdown before the restart.
    let _ = state.shutdown.send(true);

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
        // Release metadata and binaries are only trusted over TLS; `https_only`
        // also rejects redirects that downgrade to plain HTTP.
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .context("failed to build update HTTP client")
}

async fn fetch_release(client: &Client, source: &str) -> Result<GithubRelease> {
    let url = api_url(source)?;
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

fn api_url(source: &str) -> Result<String> {
    if let Ok(base) = std::env::var("ONEAPI_UPDATE_API_BASE") {
        if !base.trim().is_empty() {
            return validate_api_base(&base);
        }
    }

    let github = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest");
    Ok(if source == "ghproxy" {
        format!("{PROXY_PREFIX}{github}")
    } else {
        github
    })
}

/// `ONEAPI_UPDATE_API_BASE` moves the entire update trust boundary to that
/// endpoint, so it must not be plaintext.
fn validate_api_base(base: &str) -> Result<String> {
    let base = base.trim();
    if !base.starts_with("https://") {
        bail!("ONEAPI_UPDATE_API_BASE must start with https://");
    }
    Ok(base.to_string())
}

/// Defense in depth: the asset name is generated from a semver tag and matched
/// exactly against release metadata, but it is also used as a local file name.
fn validate_asset_name(name: &str) -> Result<()> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || Path::new(name).file_name().and_then(|part| part.to_str()) != Some(name);
    if invalid {
        bail!("refusing to use update asset name `{name}`");
    }
    Ok(())
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
