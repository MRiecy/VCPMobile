use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tauri::{ipc::Channel, AppHandle, Manager};
use tokio::io::AsyncWriteExt;

const GITHUB_API_LATEST_URL: &str = "https://api.github.com/repos/MRiecy/VCPMobile/releases/latest";
const GITHUB_API_LIST_URL: &str =
    "https://api.github.com/repos/MRiecy/VCPMobile/releases?per_page=1";
const APK_ASSET_SUFFIX: &str = "arm64-v8a.apk";
const APK_FILENAME: &str = "update.apk";
const APK_PART_FILENAME: &str = "update.apk.part";
const INSTALLING_APK_PREFIX: &str = "installing-";
const STALE_INSTALLER_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;
const GITHUB_RELEASE_PATH_PREFIX: &str = "/MRiecy/VCPMobile/releases/download/";
const LEGACY_FRONTEND_UPDATES_DIR: &str = "frontend_updates";
const LEGACY_FRONTEND_DOWNLOADS_DIR: &str = "frontend_update_downloads";
static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn is_trusted_github_download_host(host: &str) -> bool {
    matches!(
        host,
        "github.com"
            | "objects.githubusercontent.com"
            | "github-releases.githubusercontent.com"
            | "release-assets.githubusercontent.com"
    )
}

fn validate_initial_apk_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("更新下载地址无效: {error}"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.path().starts_with(GITHUB_RELEASE_PATH_PREFIX)
        || !url.path().ends_with(".apk")
        || url.username() != ""
        || url.password().is_some()
    {
        return Err("更新下载地址必须是 VCPMobile GitHub Release 的 HTTPS APK".to_string());
    }
    Ok(url)
}

fn build_update_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .redirect(Policy::custom(|attempt| {
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
        }))
        .build()
        .map_err(|error| error.to_string())
}

fn checked_download_size(downloaded: u64, chunk_len: usize) -> Result<u64, String> {
    let next = downloaded
        .checked_add(chunk_len as u64)
        .ok_or_else(|| "更新包大小溢出".to_string())?;
    if next > MAX_APK_BYTES {
        return Err(format!(
            "更新包超过 {} MiB 下载上限",
            MAX_APK_BYTES / 1024 / 1024
        ));
    }
    Ok(next)
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
    let canonical_candidate = std::fs::canonicalize(&candidate)
        .map_err(|error| format!("规范化遗留目录失败: {error}"))?;
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

fn cleanup_stale_installer_apks(cache_dir: &Path) -> Result<u32, String> {
    cleanup_stale_installer_apks_at(cache_dir, SystemTime::now())
}

fn cleanup_stale_installer_apks_at(cache_dir: &Path, now: SystemTime) -> Result<u32, String> {
    let entries = match std::fs::read_dir(cache_dir) {
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

/// 前端资源热更新已停用。应用始终使用 APK 内嵌资源；这里仅在固定目录通过
/// canonical 校验后，尽力清理旧版本留下的资源与下载缓存。
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

    match app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("获取应用缓存目录失败: {error}"))
        .and_then(|cache_dir| cleanup_stale_installer_apks(&cache_dir))
    {
        Ok(removed) if removed > 0 => {
            log::info!("[Updater] Removed {removed} stale installer APK(s)")
        }
        Ok(_) => {}
        Err(error) => log::warn!("[Updater] Stale installer cleanup skipped: {error}"),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub download_url: Option<String>,
    pub release_page_url: Option<String>,
    pub release_notes: Option<String>,
    pub apk_size: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

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

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let current_version_str = app.package_info().version.to_string();

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let release = fetch_latest_release(&client).await?;
    let latest_version = release.tag_name.trim_start_matches('v').to_string();

    let has_update = match semver::Version::parse(&latest_version) {
        Ok(latest) => match semver::Version::parse(&current_version_str) {
            Ok(current) => latest > current,
            Err(_) => latest_version != current_version_str,
        },
        Err(_) => latest_version != current_version_str,
    };

    let apk_asset = release
        .assets
        .iter()
        .find(|a| a.name.contains(APK_ASSET_SUFFIX));

    if apk_asset.is_none() && has_update {
        return Err(format!(
            "检测到新版本 {}，但该 Release 未包含 {} 安装包。\n请前往 Release 页面手动下载。",
            latest_version, APK_ASSET_SUFFIX
        ));
    }
    if let Some(asset) = apk_asset {
        if asset.size > MAX_APK_BYTES {
            return Err(format!(
                "Release APK 大小 {} MiB 超过客户端 {} MiB 上限",
                asset.size / 1024 / 1024,
                MAX_APK_BYTES / 1024 / 1024
            ));
        }
        validate_initial_apk_url(&asset.browser_download_url)?;
    }

    Ok(UpdateInfo {
        has_update,
        current_version: current_version_str,
        latest_version,
        download_url: apk_asset.map(|a| a.browser_download_url.clone()),
        release_page_url: Some(release.html_url),
        release_notes: release.body,
        apk_size: apk_asset.map(|a| a.size),
    })
}

#[tauri::command]
pub async fn download_update(
    app: AppHandle,
    url: String,
    on_progress: Channel<DownloadProgress>,
) -> Result<String, String> {
    let _download_guard = DOWNLOAD_LOCK
        .try_lock()
        .map_err(|_| "已有更新下载正在进行".to_string())?;
    let url = validate_initial_apk_url(&url)?;
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("获取缓存目录失败: {}", e))?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|error| format!("创建更新缓存目录失败: {error}"))?;
    let apk_path = cache_dir.join(APK_FILENAME);
    let part_path = cache_dir.join(APK_PART_FILENAME);
    if let Err(error) = tokio::fs::remove_file(&part_path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("清理旧更新临时文件失败: {error}"));
        }
    }

    let download_result = async {
        let client = build_update_client(Duration::from_secs(300))?;
        let res = client
            .get(url)
            .header("User-Agent", "VCPMobile")
            .send()
            .await
            .map_err(|e| format!("下载请求失败: {e}"))?;

        if !res.status().is_success() {
            return Err(format!("下载失败 ({})", res.status().as_u16()));
        }

        let total = res.content_length();
        if total.is_some_and(|size| size > MAX_APK_BYTES) {
            return Err(format!(
                "更新包超过 {} MiB 下载上限",
                MAX_APK_BYTES / 1024 / 1024
            ));
        }

        let mut downloaded: u64 = 0;
        let mut stream = res.bytes_stream();
        let mut file = tokio::fs::File::create(&part_path)
            .await
            .map_err(|e| format!("创建更新临时文件失败: {e}"))?;

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| format!("下载流错误: {e}"))?;
            downloaded = checked_download_size(downloaded, chunk.len())?;
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("写入更新临时文件失败: {e}"))?;
            let _ = on_progress.send(DownloadProgress { downloaded, total });
        }

        file.flush()
            .await
            .map_err(|e| format!("刷新更新临时文件失败: {e}"))?;
        file.sync_all()
            .await
            .map_err(|e| format!("同步更新临时文件失败: {e}"))?;
        if let Some(expected) = total {
            if downloaded != expected {
                return Err("下载文件不完整，请重试".to_string());
            }
        }
        Ok::<(), String>(())
    }
    .await;

    if let Err(error) = download_result {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::remove_file(&apk_path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(format!("替换旧更新包失败: {error}"));
        }
    }
    tokio::fs::rename(&part_path, &apk_path)
        .await
        .map_err(|error| format!("激活已下载更新包失败: {error}"))?;

    Ok(apk_path.to_string_lossy().to_string())
}

async fn stage_installer_apk_in_cache(
    cache_dir: &Path,
    requested_path: &str,
) -> Result<std::path::PathBuf, String> {
    let validated = validate_installer_path_in_cache(cache_dir, requested_path).await?;
    let staged = cache_dir.join(format!(
        "{INSTALLING_APK_PREFIX}{}.apk",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::rename(&validated, &staged)
        .await
        .map_err(|error| format!("锁定安装包版本失败: {error}"))?;
    Ok(staged)
}

async fn validate_installer_path_in_cache(
    cache_dir: &Path,
    requested_path: &str,
) -> Result<std::path::PathBuf, String> {
    let expected_path = cache_dir.join(APK_FILENAME);
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
pub async fn install_update(app: AppHandle, apk_path: String) -> Result<(), String> {
    let _download_guard = DOWNLOAD_LOCK
        .try_lock()
        .map_err(|_| "更新下载或安装正在进行".to_string())?;
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("获取缓存目录失败: {error}"))?;
    let apk_path = stage_installer_apk_in_cache(&cache_dir, &apk_path).await?;
    let apk_path_string = apk_path.to_string_lossy().to_string();
    #[cfg(target_os = "android")]
    {
        let result =
            tauri_plugin_vcp_mobile::system::open_file_native(app.clone(), apk_path_string.clone());
        if result.is_err() {
            let _ = tokio::fs::remove_file(&apk_path).await;
        }
        return result;
    }

    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt;

        // 尝试用 opener 打开本地 APK 触发系统安装器
        let result = app.opener().open_path(
            &apk_path_string,
            Some("application/vnd.android.package-archive"),
        );

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                // 删除失败的缓存文件
                let _ = tokio::fs::remove_file(&apk_path).await;
                Err(format!(
                    "无法启动安装器: {}。建议前往 GitHub Release 页面手动下载安装。",
                    e
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        checked_download_size, cleanup_stale_installer_apks_at, remove_canonical_legacy_dir,
        stage_installer_apk_in_cache, validate_initial_apk_url, validate_installer_path_in_cache,
        APK_FILENAME, MAX_APK_BYTES, STALE_INSTALLER_AGE,
    };

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
    fn apk_download_accepts_only_the_project_release_url_and_enforces_budget() {
        assert!(validate_initial_apk_url(
            "https://github.com/MRiecy/VCPMobile/releases/download/v1.2.3/VCPMobile_v1.2.3_arm64-v8a.apk"
        )
        .is_ok());
        assert!(validate_initial_apk_url(
            "http://github.com/MRiecy/VCPMobile/releases/download/v1/x.apk"
        )
        .is_err());
        assert!(validate_initial_apk_url(
            "https://example.com/MRiecy/VCPMobile/releases/download/v1/x.apk"
        )
        .is_err());
        assert!(validate_initial_apk_url(
            "https://github.com/other/repo/releases/download/v1/x.apk"
        )
        .is_err());
        assert_eq!(checked_download_size(10, 20).unwrap(), 30);
        assert!(checked_download_size(MAX_APK_BYTES, 1).is_err());
    }

    #[tokio::test]
    async fn installer_accepts_only_the_fixed_cache_apk() {
        let root = tempfile::tempdir().expect("temp root");
        let cache = root.path().join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        let expected = cache.join(APK_FILENAME);
        let outside = root.path().join("outside.apk");
        std::fs::write(&expected, b"apk").expect("expected apk");
        std::fs::write(&outside, b"apk").expect("outside apk");

        assert_eq!(
            validate_installer_path_in_cache(&cache, expected.to_str().unwrap())
                .await
                .unwrap(),
            std::fs::canonicalize(&expected).unwrap()
        );
        assert!(
            validate_installer_path_in_cache(&cache, outside.to_str().unwrap())
                .await
                .is_err()
        );

        let staged = stage_installer_apk_in_cache(&cache, expected.to_str().unwrap())
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
        assert_eq!(cleanup_stale_installer_apks_at(&cache, now).unwrap(), 0);
        assert!(staged.exists());

        let old_modified = now - STALE_INSTALLER_AGE - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&staged)
            .expect("open staged installer")
            .set_times(std::fs::FileTimes::new().set_modified(old_modified))
            .expect("age staged installer");
        assert_eq!(cleanup_stale_installer_apks_at(&cache, now).unwrap(), 1);
        assert!(!staged.exists());
        assert!(expected.exists());
    }
}
