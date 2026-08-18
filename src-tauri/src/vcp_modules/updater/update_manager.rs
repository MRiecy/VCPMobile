//! OTA 更新会话管理：状态机、Release 查询、下载编排与安装。
//!
//! 状态唯一持有者是 [`UpdateSession`]（`app.manage()` 注入），每次跃迁通过
//! `vcp-update://status` 事件广播；前端不再向下载/安装命令传递 URL 或路径。

use super::download::{self, DownloadAttempt};
use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub const UPDATE_STATUS_EVENT: &str = "vcp-update://status";

const GITHUB_API_LATEST_URL: &str = "https://api.github.com/repos/MRiecy/VCPMobile/releases/latest";
const GITHUB_API_LIST_URL: &str =
    "https://api.github.com/repos/MRiecy/VCPMobile/releases?per_page=1";
const APK_ASSET_SUFFIX: &str = "arm64-v8a.apk";
const CHECKSUM_ASSET_SUFFIX: &str = "arm64-v8a.apk.sha256";
const UPDATES_DIR_NAME: &str = "updates";
const APK_FILENAME: &str = "update.apk";
const APK_PART_FILENAME: &str = "update.apk.part";
const INSTALLING_APK_PREFIX: &str = "installing-";
const STALE_INSTALLER_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;
const GITHUB_RELEASE_PATH_PREFIX: &str = "/MRiecy/VCPMobile/releases/download/";
const LEGACY_FRONTEND_UPDATES_DIR: &str = "frontend_updates";
const LEGACY_FRONTEND_DOWNLOADS_DIR: &str = "frontend_update_downloads";
const MAX_DOWNLOAD_ATTEMPTS: u32 = 3;
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);
static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ==========================================================================
// 状态机类型
// ==========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    Verifying,
    ReadyToInstall,
    Installing,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateError {
    /// "check" | "download" | "verify" | "install"
    pub stage: String,
    pub message: String,
    pub retryable: bool,
}

impl UpdateError {
    fn new(stage: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            stage: stage.to_string(),
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_page_url: Option<String>,
    pub release_notes: Option<String>,
    pub apk_size: Option<u64>,
    pub apk_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub state: UpdateState,
    pub info: Option<UpdateInfo>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<UpdateError>,
}

#[derive(Debug, Clone)]
struct PendingArtifact {
    url: Url,
    sha256: String,
    size: u64,
}

struct SessionInner {
    state: UpdateState,
    info: Option<UpdateInfo>,
    total: Option<u64>,
    error: Option<UpdateError>,
    artifact: Option<PendingArtifact>,
    cancel: Option<Arc<AtomicBool>>,
    downloaded: Arc<AtomicU64>,
}

/// OTA 会话状态机单例。
pub struct UpdateSession {
    inner: tokio::sync::RwLock<SessionInner>,
}

impl UpdateSession {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::RwLock::new(SessionInner {
                state: UpdateState::Idle,
                info: None,
                total: None,
                error: None,
                artifact: None,
                cancel: None,
                downloaded: Arc::new(AtomicU64::new(0)),
            }),
        }
    }

    async fn snapshot(&self) -> UpdateStatus {
        let inner = self.inner.read().await;
        UpdateStatus {
            state: inner.state,
            info: inner.info.clone(),
            downloaded: inner.downloaded.load(Ordering::Relaxed),
            total: inner.total,
            error: inner.error.clone(),
        }
    }
}

fn emit_status(app: &AppHandle, status: &UpdateStatus) {
    if let Err(error) = app.emit(UPDATE_STATUS_EVENT, status) {
        log::warn!("[Updater] Failed to emit update status: {error}");
    }
}

/// 修改会话状态、广播快照并返回之。
async fn transition(
    app: &AppHandle,
    session: &UpdateSession,
    mutate: impl FnOnce(&mut SessionInner),
) -> UpdateStatus {
    let status = {
        let mut inner = session.inner.write().await;
        mutate(&mut inner);
        UpdateStatus {
            state: inner.state,
            info: inner.info.clone(),
            downloaded: inner.downloaded.load(Ordering::Relaxed),
            total: inner.total,
            error: inner.error.clone(),
        }
    };
    emit_status(app, &status);
    status
}

async fn transition_failed(
    app: &AppHandle,
    session: &UpdateSession,
    error: UpdateError,
) -> UpdateStatus {
    transition(app, session, move |inner| {
        inner.state = UpdateState::Failed;
        inner.error = Some(error);
    })
    .await
}

// ==========================================================================
// GitHub Release 查询与资产选择
// ==========================================================================

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: Option<String>,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// 只接受以 `arm64-v8a.apk` 结尾的资产；`.apk.sha256` 旁车文件不会误命中。
fn select_apk_asset(assets: &[GitHubAsset]) -> Option<&GitHubAsset> {
    assets.iter().find(|a| a.name.ends_with(APK_ASSET_SUFFIX))
}

fn select_checksum_asset<'a>(assets: &'a [GitHubAsset], apk_name: &str) -> Option<&'a GitHubAsset> {
    let expected = format!("{apk_name}.sha256");
    assets.iter().find(|a| a.name == expected)
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => latest != current,
    }
}

/// 严格解析 `sha256sum` 标准格式：`<64hex>  <文件名>`（兼容二进制模式的 `*文件名`）。
fn parse_sha256_sidecar(content: &str, expected_name: &str) -> Result<String, String> {
    let mut parts = content.trim().split_whitespace();
    let hash = parts.next().ok_or_else(|| "SHA-256 校验文件为空".to_string())?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("SHA-256 校验文件哈希格式无效".to_string());
    }
    let name = parts
        .next()
        .ok_or_else(|| "SHA-256 校验文件缺少文件名".to_string())?;
    let name = name.strip_prefix('*').unwrap_or(name);
    if name != expected_name {
        return Err(format!("SHA-256 校验文件名 {name} 与 APK {expected_name} 不一致"));
    }
    if parts.next().is_some() {
        return Err("SHA-256 校验文件包含多余内容".to_string());
    }
    Ok(hash.to_ascii_lowercase())
}

fn is_trusted_github_download_host(host: &str) -> bool {
    matches!(
        host,
        "github.com"
            | "objects.githubusercontent.com"
            | "github-releases.githubusercontent.com"
            | "release-assets.githubusercontent.com"
    )
}

fn validate_release_asset_url(raw: &str, required_suffix: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("更新下载地址无效: {error}"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.path().starts_with(GITHUB_RELEASE_PATH_PREFIX)
        || !url.path().ends_with(required_suffix)
        || url.username() != ""
        || url.password().is_some()
    {
        return Err("更新下载地址必须是 VCPMobile GitHub Release 的 HTTPS 资产".to_string());
    }
    Ok(url)
}

fn github_redirect_policy() -> Policy {
    Policy::custom(|attempt| {
        let trusted = attempt.url().scheme() == "https"
            && attempt
                .url()
                .host_str()
                .is_some_and(is_trusted_github_download_host)
            && attempt.url().username().is_empty()
            && attempt.url().password().is_none();
        if trusted && attempt.previous().len() < 5 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// API/小文件请求客户端：连接 15s + 整体超时。
fn build_update_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .redirect(github_redirect_policy())
        .build()
        .map_err(|error| error.to_string())
}

/// APK 下载客户端：只保留连接超时，整体时长由停滞判死与重试编排控制。
fn build_download_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(github_redirect_policy())
        .build()
        .map_err(|error| error.to_string())
}

async fn fetch_latest_release(client: &Client) -> Result<GitHubRelease, String> {
    // 1. 先尝试 /releases/latest（只包含正式版）
    let res = client
        .get(GITHUB_API_LATEST_URL)
        .header("User-Agent", "VCPMobile")
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    if res.status().is_success() {
        return res
            .json::<GitHubRelease>()
            .await
            .map_err(|e| format!("解析 GitHub 响应失败: {}", e));
    }

    // 2. /latest 404 时（如最新是 prerelease），降级到 /releases 列表取第一个
    if res.status().as_u16() == 404 {
        let list_res = client
            .get(GITHUB_API_LIST_URL)
            .header("User-Agent", "VCPMobile")
            .send()
            .await
            .map_err(|e| format!("网络请求失败: {}", e))?;

        if !list_res.status().is_success() {
            let status = list_res.status();
            let text = list_res.text().await.unwrap_or_default();
            return Err(format!("GitHub API 错误 ({}): {}", status.as_u16(), text));
        }

        let releases: Vec<GitHubRelease> = list_res
            .json()
            .await
            .map_err(|e| format!("解析 GitHub 响应失败: {}", e))?;

        return releases
            .into_iter()
            .next()
            .ok_or_else(|| "GitHub 上暂无任何 Release".to_string());
    }

    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    Err(format!("GitHub API 错误 ({}): {}", status.as_u16(), text))
}

async fn fetch_apk_checksum(client: &Client, asset: &GitHubAsset, apk_name: &str) -> Result<String, String> {
    let url = validate_release_asset_url(&asset.browser_download_url, CHECKSUM_ASSET_SUFFIX)?;
    let res = client
        .get(url)
        .header("User-Agent", "VCPMobile")
        .send()
        .await
        .map_err(|e| format!("获取 SHA-256 校验文件失败: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("获取 SHA-256 校验文件失败 ({})", res.status().as_u16()));
    }
    let text = res
        .text()
        .await
        .map_err(|e| format!("读取 SHA-256 校验文件失败: {e}"))?;
    parse_sha256_sidecar(&text, apk_name)
}

// ==========================================================================
// 更新目录与遗留文件管理
// ==========================================================================

fn updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("获取缓存目录失败: {e}"))?;
    Ok(cache_dir.join(UPDATES_DIR_NAME))
}

async fn ensure_updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = updates_dir(app)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| format!("创建更新缓存目录失败: {error}"))?;
    Ok(dir)
}

async fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除文件 {} 失败: {error}", path.display())),
    }
}

/// 把旧版本留在 cache 根目录的 OTA 文件迁入 `updates/` 子目录。
/// 只处理固定文件名与 `installing-*.apk` 前缀，best-effort。
fn migrate_update_cache_dir(cache_dir: &Path) {
    let updates_dir = cache_dir.join(UPDATES_DIR_NAME);
    let mut candidates: Vec<PathBuf> = [APK_FILENAME, APK_PART_FILENAME]
        .iter()
        .map(|name| cache_dir.join(name))
        .filter(|path| path.is_file())
        .collect();

    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(INSTALLING_APK_PREFIX)
                && name.ends_with(".apk")
                && entry.path().is_file()
            {
                candidates.push(entry.path());
            }
        }
    }

    if candidates.is_empty() {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(&updates_dir) {
        log::warn!("[Updater] 创建 updates 目录失败，跳过迁移: {error}");
        return;
    }
    for source in candidates {
        let Some(file_name) = source.file_name() else {
            continue;
        };
        let target = updates_dir.join(file_name);
        match std::fs::rename(&source, &target) {
            Ok(()) => log::info!("[Updater] 迁移遗留更新文件: {}", source.display()),
            Err(error) => {
                log::warn!("[Updater] 迁移 {} 失败: {error}", source.display())
            }
        }
    }
}

fn remove_canonical_legacy_dir(base_dir: &Path, leaf_name: &str) -> Result<bool, String> {
    let candidate = base_dir.join(leaf_name);
    let metadata = match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取遗留目录失败: {error}")),
    };

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "拒绝清理非普通目录的前端 OTA 遗留路径: {}",
            candidate.display()
        ));
    }

    let canonical_base =
        std::fs::canonicalize(base_dir).map_err(|error| format!("规范化应用目录失败: {error}"))?;
    let canonical_candidate =
        std::fs::canonicalize(&candidate).map_err(|error| format!("规范化遗留目录失败: {error}"))?;
    if canonical_candidate.parent() != Some(canonical_base.as_path())
        || canonical_candidate.file_name() != Some(std::ffi::OsStr::new(leaf_name))
    {
        return Err(format!(
            "拒绝清理越出应用目录的前端 OTA 遗留路径: {}",
            canonical_candidate.display()
        ));
    }

    std::fs::remove_dir_all(&canonical_candidate)
        .map_err(|error| format!("清理前端 OTA 遗留目录失败: {error}"))?;
    Ok(true)
}

fn cleanup_stale_installer_apks(updates_dir: &Path) -> Result<u32, String> {
    cleanup_stale_installer_apks_at(updates_dir, SystemTime::now())
}

fn cleanup_stale_installer_apks_at(updates_dir: &Path, now: SystemTime) -> Result<u32, String> {
    let entries = match std::fs::read_dir(updates_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("读取安装器暂存目录失败: {error}")),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取安装器暂存项失败: {error}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(uuid_text) = name
            .strip_prefix(INSTALLING_APK_PREFIX)
            .and_then(|name| name.strip_suffix(".apk"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(uuid_text).is_err() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("读取安装器暂存文件失败: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .map_err(|error| format!("读取安装器暂存时间失败: {error}"))?;
        if !now
            .duration_since(modified)
            .is_ok_and(|age| age >= STALE_INSTALLER_AGE)
        {
            continue;
        }
        std::fs::remove_file(entry.path())
            .map_err(|error| format!("清理安装器暂存文件失败: {error}"))?;
        removed += 1;
    }
    Ok(removed)
}

use std::time::SystemTime;

/// 前端资源热更新已停用。应用始终使用 APK 内嵌资源；这里在固定目录通过
/// canonical 校验后尽力清理旧版本遗留，同时把旧位置的更新包迁入 `updates/`。
pub fn cleanup_legacy_frontend_ota(app: &AppHandle) {
    let legacy_dirs = [
        (app.path().app_config_dir(), LEGACY_FRONTEND_UPDATES_DIR),
        (app.path().app_cache_dir(), LEGACY_FRONTEND_DOWNLOADS_DIR),
    ];

    for (base_dir, leaf_name) in legacy_dirs {
        let result = base_dir
            .map_err(|error| format!("获取应用目录失败: {error}"))
            .and_then(|base_dir| remove_canonical_legacy_dir(&base_dir, leaf_name));
        match result {
            Ok(true) => log::info!("[Updater] Removed legacy frontend OTA directory: {leaf_name}"),
            Ok(false) => {}
            Err(error) => log::warn!(
                "[Updater] Legacy frontend OTA directory was not removed and will not be read: {error}"
            ),
        }
    }

    if let Ok(cache_dir) = app.path().app_cache_dir() {
        migrate_update_cache_dir(&cache_dir);
        match cleanup_stale_installer_apks(&cache_dir.join(UPDATES_DIR_NAME)) {
            Ok(removed) if removed > 0 => {
                log::info!("[Updater] Removed {removed} stale installer APK(s)")
            }
            Ok(_) => {}
            Err(error) => log::warn!("[Updater] Stale installer cleanup skipped: {error}"),
        }
    }
}

// ==========================================================================
// Tauri 命令
// ==========================================================================

#[tauri::command]
pub async fn get_update_status(session: State<'_, UpdateSession>) -> Result<UpdateStatus, String> {
    Ok(session.snapshot().await)
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    session: State<'_, UpdateSession>,
) -> Result<UpdateStatus, String> {
    transition(&app, &session, |inner| {
        inner.state = UpdateState::Checking;
        inner.error = None;
    })
    .await;

    let outcome = async {
        let client = build_update_client(Duration::from_secs(15))?;
        let current_version = app.package_info().version.to_string();
        let release = fetch_latest_release(&client).await?;
        let latest_version = release.tag_name.trim_start_matches('v').to_string();

        let base_info = UpdateInfo {
            has_update: false,
            current_version: current_version.clone(),
            latest_version: latest_version.clone(),
            release_page_url: Some(release.html_url.clone()),
            release_notes: release.body.clone(),
            apk_size: None,
            apk_sha256: None,
        };

        if !version_is_newer(&latest_version, &current_version) {
            return Ok((base_info, None));
        }

        let apk_asset = select_apk_asset(&release.assets).ok_or_else(|| {
            format!(
                "检测到新版本 {latest_version}，但该 Release 未包含 {APK_ASSET_SUFFIX} 安装包。\n请前往 Release 页面手动下载。"
            )
        })?;
        if apk_asset.size > MAX_APK_BYTES {
            return Err(format!(
                "Release APK 大小 {} MiB 超过客户端 {} MiB 上限",
                apk_asset.size / 1024 / 1024,
                MAX_APK_BYTES / 1024 / 1024
            ));
        }
        let checksum_asset = select_checksum_asset(&release.assets, &apk_asset.name)
            .ok_or_else(|| "Release 缺少配套的 SHA-256 校验文件".to_string())?;
        let sha256 = fetch_apk_checksum(&client, checksum_asset, &apk_asset.name).await?;
        let url = validate_release_asset_url(&apk_asset.browser_download_url, APK_ASSET_SUFFIX)?;

        let artifact = PendingArtifact {
            url,
            sha256: sha256.clone(),
            size: apk_asset.size,
        };
        let info = UpdateInfo {
            has_update: true,
            apk_size: Some(apk_asset.size),
            apk_sha256: Some(sha256),
            ..base_info
        };
        Ok((info, Some(artifact)))
    }
    .await;

    let (info, artifact) = match outcome {
        Ok(value) => value,
        Err(message) => {
            let status = transition_failed(
                &app,
                &session,
                UpdateError::new("check", message, true),
            )
            .await;
            return Ok(status);
        }
    };

    let Some(artifact) = artifact else {
        // 已是最新
        let status = transition(&app, &session, move |inner| {
            inner.state = UpdateState::Idle;
            inner.info = Some(info);
            inner.error = None;
            inner.artifact = None;
            inner.downloaded.store(0, Ordering::Relaxed);
        })
        .await;
        return Ok(status);
    };

    // 本地已有完整包且校验通过 → 直接可安装，消除重复下载
    let updates = ensure_updates_dir(&app).await?;
    let apk_path = updates.join(APK_FILENAME);
    if apk_path.is_file() {
        match download::verify_file_sha256(&apk_path, &artifact.sha256).await {
            Ok(true) => {
                let status = transition(&app, &session, move |inner| {
                    inner.state = UpdateState::ReadyToInstall;
                    inner.info = Some(info);
                    inner.error = None;
                    inner.artifact = Some(artifact.clone());
                    inner.total = Some(artifact.size);
                    inner.downloaded.store(artifact.size, Ordering::Relaxed);
                })
                .await;
                return Ok(status);
            }
            Ok(false) => {
                log::info!("[Updater] 本地更新包校验和不匹配，删除后重新下载");
                remove_file_if_exists(&apk_path).await?;
            }
            Err(error) => {
                log::warn!("[Updater] 本地更新包校验失败，删除后重新下载: {error}");
                remove_file_if_exists(&apk_path).await?;
            }
        }
    }

    // 过期的残断文件不应参与续传
    let part_path = updates.join(APK_PART_FILENAME);
    let part_len = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);
    if part_len > artifact.size {
        remove_file_if_exists(&part_path).await?;
    }

    let status = transition(&app, &session, move |inner| {
        inner.state = UpdateState::Available;
        inner.info = Some(info);
        inner.error = None;
        inner.artifact = Some(artifact.clone());
        inner.total = Some(artifact.size);
        inner.downloaded.store(0, Ordering::Relaxed);
    })
    .await;
    Ok(status)
}

#[tauri::command]
pub async fn start_update_download(
    app: AppHandle,
    session: State<'_, UpdateSession>,
) -> Result<UpdateStatus, String> {
    let artifact = {
        let inner = session.inner.read().await;
        match inner.state {
            UpdateState::ReadyToInstall => return Ok(session.snapshot().await),
            UpdateState::Downloading | UpdateState::Verifying => {
                return Err("已有更新下载正在进行".to_string())
            }
            _ => inner
                .artifact
                .clone()
                .ok_or_else(|| "没有可下载的更新，请先检查更新".to_string())?,
        }
    };

    let _download_guard = DOWNLOAD_LOCK
        .try_lock()
        .map_err(|_| "已有更新下载正在进行".to_string())?;

    // 下载期间持前台锁，防止切后台后进程被杀；保活失败不阻断下载。
    if let Err(error) = tauri_plugin_vcp_mobile::system::acquire_ota_keepalive(app.clone()) {
        log::warn!("[Updater] OTA 保活获取失败，下载将继续但不保活: {error}");
    }
    let result = run_update_download(app.clone(), session, artifact).await;
    if let Err(error) = tauri_plugin_vcp_mobile::system::release_ota_keepalive(app.clone()) {
        log::warn!("[Updater] OTA 保活释放失败: {error}");
    }
    result
}

async fn run_update_download(
    app: AppHandle,
    session: State<'_, UpdateSession>,
    artifact: PendingArtifact,
) -> Result<UpdateStatus, String> {
    let updates = ensure_updates_dir(&app).await?;
    let part_path = updates.join(APK_PART_FILENAME);
    let apk_path = updates.join(APK_FILENAME);

    let cancel = Arc::new(AtomicBool::new(false));
    let downloaded_arc = {
        let mut inner = session.inner.write().await;
        inner.cancel = Some(cancel.clone());
        inner.downloaded.store(0, Ordering::Relaxed);
        inner.downloaded.clone()
    };

    let client = build_download_client()?;
    let mut last_error = String::new();
    let mut completed = false;

    for attempt in 0..MAX_DOWNLOAD_ATTEMPTS {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
        }

        transition(&app, &session, |inner| {
            inner.state = UpdateState::Downloading;
            inner.error = None;
            inner.total = Some(artifact.size);
        })
        .await;

        let progress_app = app.clone();
        let progress_info = session
            .inner
            .read()
            .await
            .info
            .clone();
        let progress_downloaded = downloaded_arc.clone();
        let total = artifact.size;
        let last_emit = std::sync::Mutex::new(Instant::now() - PROGRESS_EMIT_INTERVAL);
        let mut on_progress = move |downloaded: u64| {
            progress_downloaded.store(downloaded, Ordering::Relaxed);
            if let Ok(mut last) = last_emit.lock() {
                if last.elapsed() >= PROGRESS_EMIT_INTERVAL || downloaded >= total {
                    *last = Instant::now();
                    emit_status(
                        &progress_app,
                        &UpdateStatus {
                            state: UpdateState::Downloading,
                            info: progress_info.clone(),
                            downloaded,
                            total: Some(total),
                            error: None,
                        },
                    );
                }
            }
        };

        match download::download_once(
            &client,
            &artifact.url,
            &part_path,
            MAX_APK_BYTES,
            &cancel,
            &mut on_progress,
        )
        .await
        {
            Ok(DownloadAttempt::Completed) => {
                completed = true;
                break;
            }
            Ok(DownloadAttempt::Cancelled) => {
                let status = transition(&app, &session, |inner| {
                    inner.state = UpdateState::Available;
                    inner.error = None;
                    inner.cancel = None;
                })
                .await;
                return Ok(status);
            }
            Ok(DownloadAttempt::Retryable(reason)) => {
                last_error = reason;
                continue;
            }
            Ok(DownloadAttempt::RestartFromScratch) => {
                remove_file_if_exists(&part_path).await?;
                downloaded_arc.store(0, Ordering::Relaxed);
                last_error = "续传起点不被服务端接受，已重新下载".to_string();
                continue;
            }
            Err(fatal) => {
                let status = transition_failed(
                    &app,
                    &session,
                    UpdateError::new("download", fatal, false),
                )
                .await;
                return Ok(status);
            }
        }
    }

    if !completed {
        if cancel.load(Ordering::Relaxed) {
            let status = transition(&app, &session, |inner| {
                inner.state = UpdateState::Available;
                inner.error = None;
                inner.cancel = None;
            })
            .await;
            return Ok(status);
        }
        let status = transition_failed(
            &app,
            &session,
            UpdateError::new(
                "download",
                format!("多次尝试后下载仍未完成: {last_error}"),
                true,
            ),
        )
        .await;
        return Ok(status);
    }

    // 字节数核对：超出预期说明数据损坏，删除；不足则保留 .part 供续传重试。
    let final_len = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if final_len != artifact.size {
        if final_len > artifact.size {
            remove_file_if_exists(&part_path).await?;
        }
        let status = transition_failed(
            &app,
            &session,
            UpdateError::new("download", "下载文件不完整，请重试", true),
        )
        .await;
        return Ok(status);
    }

    transition(&app, &session, |inner| {
        inner.state = UpdateState::Verifying;
        inner.error = None;
    })
    .await;

    match download::verify_file_sha256(&part_path, &artifact.sha256).await {
        Ok(true) => {}
        Ok(false) => {
            remove_file_if_exists(&part_path).await?;
            let status = transition_failed(
                &app,
                &session,
                UpdateError::new(
                    "verify",
                    "更新包 SHA-256 校验不匹配，已删除损坏文件，请重新下载",
                    true,
                ),
            )
            .await;
            return Ok(status);
        }
        Err(error) => {
            let status =
                transition_failed(&app, &session, UpdateError::new("verify", error, true)).await;
            return Ok(status);
        }
    }

    remove_file_if_exists(&apk_path).await?;
    tokio::fs::rename(&part_path, &apk_path)
        .await
        .map_err(|error| format!("激活已下载更新包失败: {error}"))?;

    let status = transition(&app, &session, |inner| {
        inner.state = UpdateState::ReadyToInstall;
        inner.error = None;
        inner.cancel = None;
        inner.downloaded.store(artifact.size, Ordering::Relaxed);
    })
    .await;
    Ok(status)
}

#[tauri::command]
pub async fn cancel_update_download(
    _app: AppHandle,
    session: State<'_, UpdateSession>,
) -> Result<UpdateStatus, String> {
    let flag = session.inner.read().await.cancel.clone();
    if let Some(flag) = flag {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(session.snapshot().await)
}

/// 安装类错误：保持 ReadyToInstall（可重试），仅记录错误。
async fn transition_install_error(
    app: &AppHandle,
    session: &UpdateSession,
    message: String,
    retryable: bool,
) -> UpdateStatus {
    transition(app, session, move |inner| {
        inner.state = UpdateState::ReadyToInstall;
        inner.error = Some(UpdateError::new("install", message, retryable));
    })
    .await
}

async fn stage_installer_apk_in_updates(
    updates_dir: &Path,
    requested_path: &Path,
) -> Result<PathBuf, String> {
    let validated = validate_installer_path_in_updates(updates_dir, requested_path).await?;
    let staged = updates_dir.join(format!(
        "{INSTALLING_APK_PREFIX}{}.apk",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::rename(&validated, &staged)
        .await
        .map_err(|error| format!("锁定安装包版本失败: {error}"))?;
    Ok(staged)
}

async fn validate_installer_path_in_updates(
    updates_dir: &Path,
    requested_path: &Path,
) -> Result<PathBuf, String> {
    let expected_path = updates_dir.join(APK_FILENAME);
    let metadata = tokio::fs::symlink_metadata(requested_path)
        .await
        .map_err(|error| format!("读取更新包元数据失败: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("更新包必须是应用缓存中的普通 APK 文件".to_string());
    }
    let requested = tokio::fs::canonicalize(requested_path)
        .await
        .map_err(|error| format!("规范化更新包路径失败: {error}"))?;
    let expected = tokio::fs::canonicalize(expected_path)
        .await
        .map_err(|error| format!("规范化应用更新包路径失败: {error}"))?;
    if requested != expected {
        return Err("拒绝安装应用更新缓存之外的路径".to_string());
    }
    Ok(expected)
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    session: State<'_, UpdateSession>,
) -> Result<UpdateStatus, String> {
    {
        let inner = session.inner.read().await;
        if inner.state != UpdateState::ReadyToInstall {
            return Err("当前没有已校验完成、可安装的更新包".to_string());
        }
    }
    let _download_guard = DOWNLOAD_LOCK
        .try_lock()
        .map_err(|_| "更新下载或安装正在进行".to_string())?;

    transition(&app, &session, |inner| {
        inner.state = UpdateState::Installing;
        inner.error = None;
    })
    .await;

    let updates = updates_dir(&app)?;
    let apk_path = updates.join(APK_FILENAME);

    // 安装前置检查（仅 Android）。检查失败时保持 ReadyToInstall 并记录错误，
    // 用户处理问题后可直接重试安装，无需重新下载。
    #[cfg(target_os = "android")]
    {
        match tauri_plugin_vcp_mobile::system::can_install_packages(app.clone()) {
            Ok(true) => {}
            Ok(false) => {
                let status = transition_install_error(
                    &app,
                    &session,
                    "未授予「安装未知应用」权限，请在系统设置中授权后重试".to_string(),
                    true,
                )
                .await;
                return Ok(status);
            }
            Err(error) => {
                let status = transition_install_error(
                    &app,
                    &session,
                    format!("检查安装权限失败: {error}"),
                    true,
                )
                .await;
                return Ok(status);
            }
        }

        match tauri_plugin_vcp_mobile::system::verify_apk_signature(
            app.clone(),
            apk_path.to_string_lossy().to_string(),
        ) {
            Ok(result) if result.matched => {}
            Ok(_) => {
                // 签名不连续：拒绝安装并销毁安装包，引导用户核实发布渠道
                remove_file_if_exists(&apk_path).await.ok();
                let status = transition(&app, &session, |inner| {
                    inner.state = UpdateState::Available;
                    inner.error = Some(UpdateError::new(
                        "install",
                        "更新包签名证书与当前应用不一致，已拒绝安装。请前往 Release 页面核实后手动下载。",
                        false,
                    ));
                })
                .await;
                return Ok(status);
            }
            Err(error) => {
                let status = transition_install_error(
                    &app,
                    &session,
                    format!("签名校验失败: {error}"),
                    true,
                )
                .await;
                return Ok(status);
            }
        }
    }

    let staged = match stage_installer_apk_in_updates(&updates, &apk_path).await {
        Ok(staged) => staged,
        Err(error) => {
            let status = transition_install_error(&app, &session, error, true).await;
            return Ok(status);
        }
    };
    let staged_string = staged.to_string_lossy().to_string();

    #[cfg(target_os = "android")]
    {
        let result =
            tauri_plugin_vcp_mobile::system::open_file_native(app.clone(), staged_string.clone());
        if let Err(error) = result {
            remove_file_if_exists(&staged).await.ok();
            let status = transition_install_error(&app, &session, error, true).await;
            return Ok(status);
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt;

        // 尝试用 opener 打开本地 APK 触发系统安装器
        let result = app.opener().open_path(
            &staged_string,
            Some("application/vnd.android.package-archive"),
        );

        if let Err(error) = result {
            remove_file_if_exists(&staged).await.ok();
            let status = transition_install_error(
                &app,
                &session,
                format!("无法启动安装器: {error}。建议前往 GitHub Release 页面手动下载安装。"),
                true,
            )
            .await;
            return Ok(status);
        }
    }

    let status = transition(&app, &session, |inner| {
        inner.state = UpdateState::Idle;
        inner.error = None;
        inner.downloaded.store(0, Ordering::Relaxed);
    })
    .await;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_stale_installer_apks_at, parse_sha256_sidecar, remove_canonical_legacy_dir,
        select_apk_asset, select_checksum_asset, stage_installer_apk_in_updates,
        validate_installer_path_in_updates, validate_release_asset_url, version_is_newer,
        GitHubAsset, APK_ASSET_SUFFIX, APK_FILENAME, STALE_INSTALLER_AGE,
    };

    fn asset(name: &str) -> GitHubAsset {
        GitHubAsset {
            name: name.to_string(),
            browser_download_url: format!(
                "https://github.com/MRiecy/VCPMobile/releases/download/v1.1.4/{name}"
            ),
            size: 100,
        }
    }

    #[test]
    fn apk_asset_selection_never_matches_the_sha256_sidecar() {
        let assets = [
            asset("VCPMobile_v1.1.4_arm64-v8a.apk.sha256"),
            asset("VCPMobile_v1.1.4_arm64-v8a.apk"),
        ];
        let selected = select_apk_asset(&assets).expect("apk asset");
        assert_eq!(selected.name, "VCPMobile_v1.1.4_arm64-v8a.apk");

        // 即使旁车文件单独存在也不会被选中
        let only_sidecar = [asset("VCPMobile_v1.1.4_arm64-v8a.apk.sha256")];
        assert!(select_apk_asset(&only_sidecar).is_none());

        let checksum = select_checksum_asset(&assets, "VCPMobile_v1.1.4_arm64-v8a.apk")
            .expect("checksum asset");
        assert_eq!(
            checksum.name,
            format!("VCPMobile_v1.1.4_arm64-v8a.apk.sha256")
        );
        assert!(select_checksum_asset(&assets, "other.apk").is_none());
        assert_eq!(APK_ASSET_SUFFIX, "arm64-v8a.apk");
    }

    #[test]
    fn sha256_sidecar_parsing_is_strict() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_sha256_sidecar(
                &format!("{hash}  VCPMobile_v1.1.4_arm64-v8a.apk\n"),
                "VCPMobile_v1.1.4_arm64-v8a.apk"
            )
            .unwrap(),
            hash
        );
        // sha256sum 二进制模式的 `*文件名` 兼容
        assert!(
            parse_sha256_sidecar(
                &format!("{hash} *VCPMobile_v1.1.4_arm64-v8a.apk"),
                "VCPMobile_v1.1.4_arm64-v8a.apk"
            )
            .is_ok()
        );
        // 大写哈希归一化为小写
        assert_eq!(
            parse_sha256_sidecar(
                &format!("{}  VCPMobile_v1.1.4_arm64-v8a.apk", "A".repeat(64)),
                "VCPMobile_v1.1.4_arm64-v8a.apk"
            )
            .unwrap(),
            "a".repeat(64)
        );
        // 文件名不匹配 / 畸形 / 多余内容全部拒绝
        assert!(parse_sha256_sidecar(&format!("{hash}  other.apk"), "VCPMobile_v1.1.4_arm64-v8a.apk").is_err());
        assert!(parse_sha256_sidecar("not-a-hash  x.apk", "x.apk").is_err());
        assert!(parse_sha256_sidecar("", "x.apk").is_err());
        assert!(parse_sha256_sidecar(&format!("{hash}"), "x.apk").is_err());
        assert!(parse_sha256_sidecar(&format!("{hash}  x.apk extra"), "x.apk").is_err());
    }

    #[test]
    fn version_comparison_prefers_semver() {
        assert!(version_is_newer("1.2.0", "1.1.4"));
        assert!(!version_is_newer("1.1.4", "1.1.4"));
        assert!(!version_is_newer("1.1.3", "1.1.4"));
        // 非 semver 退化为字符串不等
        assert!(version_is_newer("nightly-x", "1.1.4"));
        assert!(!version_is_newer("1.1.4", "1.1.4"));
    }

    #[test]
    fn legacy_frontend_ota_cleanup_removes_only_the_fixed_direct_child() {
        let root = tempfile::tempdir().expect("temp root");
        let legacy = root.path().join("frontend_updates");
        let sibling = root.path().join("keep-me");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::write(legacy.join("active_version"), b"../../keep-me")
            .expect("malicious legacy marker");
        std::fs::create_dir_all(&sibling).expect("sibling dir");

        assert!(remove_canonical_legacy_dir(root.path(), "frontend_updates").expect("safe cleanup"));
        assert!(!legacy.exists());
        assert!(sibling.exists());
    }

    #[test]
    fn legacy_frontend_ota_cleanup_is_idempotent_and_rejects_a_file() {
        let root = tempfile::tempdir().expect("temp root");

        assert!(
            !remove_canonical_legacy_dir(root.path(), "frontend_updates")
                .expect("missing legacy directory is a no-op")
        );

        let legacy_file = root.path().join("frontend_updates");
        std::fs::write(&legacy_file, b"not a directory").expect("legacy file");
        assert!(remove_canonical_legacy_dir(root.path(), "frontend_updates").is_err());
        assert_eq!(
            std::fs::read(&legacy_file).expect("legacy file remains"),
            b"not a directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_frontend_ota_cleanup_refuses_a_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp root");
        let outside = tempfile::tempdir().expect("outside root");
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"keep").expect("sentinel");
        symlink(outside.path(), root.path().join("frontend_updates")).expect("legacy symlink");

        assert!(remove_canonical_legacy_dir(root.path(), "frontend_updates").is_err());
        assert!(sentinel.exists());
    }

    #[test]
    fn release_asset_url_validation() {
        assert!(validate_release_asset_url(
            "https://github.com/MRiecy/VCPMobile/releases/download/v1.2.3/VCPMobile_v1.2.3_arm64-v8a.apk",
            "arm64-v8a.apk"
        )
        .is_ok());
        assert!(validate_release_asset_url(
            "https://github.com/MRiecy/VCPMobile/releases/download/v1.2.3/VCPMobile_v1.2.3_arm64-v8a.apk.sha256",
            "arm64-v8a.apk.sha256"
        )
        .is_ok());
        // 后缀不匹配：sha256 旁车不能当 APK 用
        assert!(validate_release_asset_url(
            "https://github.com/MRiecy/VCPMobile/releases/download/v1.2.3/VCPMobile_v1.2.3_arm64-v8a.apk.sha256",
            "arm64-v8a.apk"
        )
        .is_err());
        assert!(validate_release_asset_url(
            "http://github.com/MRiecy/VCPMobile/releases/download/v1/x.apk",
            "arm64-v8a.apk"
        )
        .is_err());
        assert!(validate_release_asset_url(
            "https://example.com/MRiecy/VCPMobile/releases/download/v1/x.apk",
            "arm64-v8a.apk"
        )
        .is_err());
        assert!(validate_release_asset_url(
            "https://github.com/other/repo/releases/download/v1/x.apk",
            "arm64-v8a.apk"
        )
        .is_err());
    }

    #[tokio::test]
    async fn installer_accepts_only_the_fixed_updates_apk() {
        let root = tempfile::tempdir().expect("temp root");
        let updates = root.path().join("updates");
        std::fs::create_dir_all(&updates).expect("updates");
        let expected = updates.join(APK_FILENAME);
        let outside = root.path().join("outside.apk");
        std::fs::write(&expected, b"apk").expect("expected apk");
        std::fs::write(&outside, b"apk").expect("outside apk");

        assert_eq!(
            validate_installer_path_in_updates(&updates, &expected)
                .await
                .unwrap(),
            std::fs::canonicalize(&expected).unwrap()
        );
        assert!(validate_installer_path_in_updates(&updates, &outside)
            .await
            .is_err());

        let staged = stage_installer_apk_in_updates(&updates, &expected)
            .await
            .expect("stage validated installer");
        assert!(!expected.exists());
        assert_eq!(std::fs::read(&staged).unwrap(), b"apk");

        // A later download only replaces update.apk; the package installer keeps a
        // generation-unique immutable path to the bytes that were validated.
        std::fs::write(&expected, b"new-apk").expect("new download generation");
        assert_eq!(std::fs::read(&staged).unwrap(), b"apk");
        assert_eq!(std::fs::read(&expected).unwrap(), b"new-apk");

        let now = std::time::SystemTime::now();
        assert_eq!(cleanup_stale_installer_apks_at(&updates, now).unwrap(), 0);
        assert!(staged.exists());

        let old_modified = now - STALE_INSTALLER_AGE - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&staged)
            .expect("open staged installer")
            .set_times(std::fs::FileTimes::new().set_modified(old_modified))
            .expect("age staged installer");
        assert_eq!(cleanup_stale_installer_apks_at(&updates, now).unwrap(), 1);
        assert!(!staged.exists());
        assert!(expected.exists());
    }
}
