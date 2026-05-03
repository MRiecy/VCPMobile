use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager};
use tokio::io::AsyncWriteExt;

const FRONTEND_DIST_PREFIX: &str = "frontend-dist-v";
const FRONTEND_DIST_SUFFIX: &str = ".zip";
const GITHUB_API_LATEST_URL: &str = "https://api.github.com/repos/MRiecy/VCPMobile/releases/latest";
const GITHUB_API_LIST_URL: &str =
    "https://api.github.com/repos/MRiecy/VCPMobile/releases?per_page=1";

/// 前端更新包元信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FrontendUpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub remote_version: String,
    pub download_url: Option<String>,
    pub zip_size: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// =================================================================
/// 内部辅助函数
/// =================================================================
fn get_frontend_updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let mut dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    dir.push("frontend_updates");
    Ok(dir)
}

fn get_active_version_path(app: &AppHandle) -> Result<PathBuf, String> {
    let mut dir = get_frontend_updates_dir(app)?;
    dir.push("active_version");
    Ok(dir)
}

fn get_boot_manifest_path(app: &AppHandle) -> Result<PathBuf, String> {
    let mut dir = get_frontend_updates_dir(app)?;
    dir.push("boot_manifest.json");
    Ok(dir)
}

/// 读取当前激活的前端版本（文件系统热更新版本）
pub fn read_active_version(app: &AppHandle) -> Option<String> {
    get_active_version_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 获取当前 APK 的内置版本号
fn get_apk_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 读取本地当前应作为 baseline 的版本号
fn get_local_baseline_version(app: &AppHandle) -> String {
    read_active_version(app).unwrap_or_else(|| get_apk_version(app))
}

async fn fetch_latest_release(client: &Client) -> Result<GitHubRelease, String> {
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

/// 查找 Release Assets 中的前端资源包
fn find_frontend_asset(release: &GitHubRelease) -> Option<&GitHubAsset> {
    release.assets.iter().find(|a| {
        a.name.starts_with(FRONTEND_DIST_PREFIX) && a.name.ends_with(FRONTEND_DIST_SUFFIX)
    })
}

/// 从文件名中提取版本号，例如 "frontend-dist-v0.9.12.zip" -> "0.9.12"
fn extract_version_from_asset_name(name: &str) -> Option<String> {
    let name = name
        .strip_prefix(FRONTEND_DIST_PREFIX)?
        .strip_suffix(FRONTEND_DIST_SUFFIX)?;
    Some(name.to_string())
}

/// 校验 zip 内 manifest.json 与解压后的文件 hash
fn verify_unzipped_files(update_dir: &Path) -> Result<(), String> {
    let manifest_path = update_dir.join("manifest.json");
    if !manifest_path.exists() {
        // 如果没有 manifest，跳过校验（向后兼容）
        return Ok(());
    }

    let manifest_content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content)
        .map_err(|e| format!("解析 manifest.json 失败: {}", e))?;

    let files = manifest
        .get("files")
        .and_then(|v| v.as_object())
        .ok_or("manifest.json 缺少 files 字段")?;

    for (relative_path, expected_hash_val) in files {
        let expected_hash = expected_hash_val
            .as_str()
            .ok_or_else(|| format!("manifest 中 {} 的 hash 不是字符串", relative_path))?;

        let file_path = update_dir.join(relative_path.trim_start_matches('/'));
        if !file_path.exists() {
            return Err(format!("校验失败: 缺少文件 {}", relative_path));
        }

        let content = std::fs::read(&file_path)
            .map_err(|e| format!("读取文件 {} 失败: {}", relative_path, e))?;
        let actual_hash = format!("{:x}", sha2::Sha256::digest(&content));

        if actual_hash != expected_hash {
            return Err(format!(
                "校验失败: {} hash 不匹配 (期望: {}, 实际: {})",
                relative_path, expected_hash, actual_hash
            ));
        }
    }

    Ok(())
}

/// 清理旧版本，只保留最近 N 个
fn cleanup_old_versions(updates_dir: &Path, keep: usize) -> Result<(), String> {
    let mut versions: Vec<(String, PathBuf)> = std::fs::read_dir(updates_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name()?.to_string_lossy().to_string();
                // 简单过滤：排除特殊文件，保留看起来像版本号的目录
                if name != "active_version" && name != "boot_manifest.json" {
                    Some((name, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    if versions.len() <= keep {
        return Ok(());
    }

    // 按 semver 排序，删除旧的
    versions.sort_by(
        |a, b| match (semver::Version::parse(&a.0), semver::Version::parse(&b.0)) {
            (Ok(va), Ok(vb)) => va.cmp(&vb),
            _ => a.0.cmp(&b.0),
        },
    );

    let to_remove = versions.len().saturating_sub(keep);
    for (_, path) in versions.into_iter().take(to_remove) {
        let _ = std::fs::remove_dir_all(&path);
        log::info!("[FrontendUpdate] Cleaned up old version: {:?}", path);
    }

    Ok(())
}

/// =================================================================
/// Tauri 指令
/// =================================================================

#[tauri::command]
pub async fn check_for_frontend_update(app: AppHandle) -> Result<FrontendUpdateInfo, String> {
    let current_version = get_local_baseline_version(&app);

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let release = fetch_latest_release(&client).await?;
    let frontend_asset = find_frontend_asset(&release);

    let remote_version = frontend_asset
        .and_then(|a| extract_version_from_asset_name(&a.name))
        .unwrap_or_else(|| release.tag_name.trim_start_matches('v').to_string());

    let has_update = match semver::Version::parse(&remote_version) {
        Ok(remote) => match semver::Version::parse(&current_version) {
            Ok(current) => remote > current,
            Err(_) => remote_version != current_version,
        },
        Err(_) => remote_version != current_version,
    };

    Ok(FrontendUpdateInfo {
        has_update,
        current_version,
        remote_version,
        download_url: frontend_asset.map(|a| a.browser_download_url.clone()),
        zip_size: frontend_asset.map(|a| a.size),
    })
}

pub(crate) async fn download_frontend_update_inner(
    app: &AppHandle,
    url: &str,
    on_progress: Option<Channel<DownloadProgress>>,
) -> Result<String, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("获取缓存目录失败: {}", e))?;

    let download_dir = cache_dir.join("frontend_update_downloads");
    if !download_dir.exists() {
        let _ = std::fs::create_dir_all(&download_dir);
    }

    let file_name = url
        .split('/')
        .next_back()
        .unwrap_or("frontend-update.zip")
        .to_string();
    let zip_path = download_dir.join(&file_name);

    if zip_path.exists() {
        let _ = tokio::fs::remove_file(&zip_path).await;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(url)
        .header("User-Agent", "VCPMobile")
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        return Err(format!("下载失败 ({})", status.as_u16()));
    }

    let total = res.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();
    let mut file = tokio::fs::File::create(&zip_path)
        .await
        .map_err(|e| format!("创建文件失败: {}", e))?;

    use futures_util::StreamExt;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("下载流错误: {}", e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入文件失败: {}", e))?;
        downloaded += chunk.len() as u64;
        if let Some(ref ch) = on_progress {
            let _ = ch.send(DownloadProgress { downloaded, total });
        }
    }

    file.flush().await.map_err(|e| e.to_string())?;

    if let Some(expected) = total {
        if downloaded != expected {
            let _ = tokio::fs::remove_file(&zip_path).await;
            return Err("下载文件不完整，请重试".to_string());
        }
    }

    Ok(zip_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn download_frontend_update(
    app: AppHandle,
    url: String,
    on_progress: Channel<DownloadProgress>,
) -> Result<String, String> {
    download_frontend_update_inner(&app, &url, Some(on_progress)).await
}

#[tauri::command]
pub async fn apply_frontend_update(
    app: AppHandle,
    zip_path: String,
    version: String,
) -> Result<(), String> {
    let updates_dir = get_frontend_updates_dir(&app)?;
    if !updates_dir.exists() {
        std::fs::create_dir_all(&updates_dir).map_err(|e| e.to_string())?;
    }

    let version_dir = updates_dir.join(&version);
    if version_dir.exists() {
        let _ = std::fs::remove_dir_all(&version_dir);
    }
    std::fs::create_dir_all(&version_dir).map_err(|e| e.to_string())?;

    // 解压 zip
    let file = std::fs::File::open(&zip_path).map_err(|e| format!("打开 zip 失败: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip 失败: {}", e))?;

    for i in 0..archive.len() {
        let mut file_in_zip = archive
            .by_index(i)
            .map_err(|e| format!("解压条目失败: {}", e))?;
        let out_path = version_dir.join(file_in_zip.name());

        if file_in_zip.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            let mut out_file = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            let mut buf = Vec::with_capacity(file_in_zip.size() as usize);
            file_in_zip
                .read_to_end(&mut buf)
                .map_err(|e| format!("读取 zip 内容失败: {}", e))?;
            out_file.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }

    // 校验
    verify_unzipped_files(&version_dir)?;

    // 写入 active_version
    let active_version_path = get_active_version_path(&app)?;
    std::fs::write(&active_version_path, &version).map_err(|e| e.to_string())?;

    // 清理旧版本（保留最近 2 个）
    let _ = cleanup_old_versions(&updates_dir, 2);

    // 删除下载的 zip
    let _ = tokio::fs::remove_file(&zip_path).await;

    log::info!("[FrontendUpdate] Applied version {} successfully", version);
    Ok(())
}

#[tauri::command]
pub async fn get_active_frontend_version(app: AppHandle) -> Result<Option<String>, String> {
    Ok(read_active_version(&app))
}

#[tauri::command]
pub async fn clear_frontend_updates(app: AppHandle) -> Result<(), String> {
    let updates_dir = get_frontend_updates_dir(&app)?;
    if updates_dir.exists() {
        let _ = std::fs::remove_dir_all(&updates_dir);
    }
    log::info!("[FrontendUpdate] Cleared all frontend updates");
    Ok(())
}

/// 前端启动成功后调用，用于回滚保护
#[tauri::command]
pub async fn confirm_frontend_boot(app: AppHandle) -> Result<(), String> {
    let manifest_path = get_boot_manifest_path(&app)?;
    let version = match read_active_version(&app) {
        Some(v) => v,
        None => return Ok(()),
    };

    let mut manifest = if manifest_path.exists() {
        let content = std::fs::read_to_string(&manifest_path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::Map::new())
    } else {
        serde_json::Map::new()
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    manifest.insert(
        version.clone(),
        serde_json::json!({
            "last_boot_at": now,
            "boot_count": manifest.get(&version)
                .and_then(|v| v.get("boot_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) + 1
        }),
    );

    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// 启动时调用：若某版本连续多次未成功 boot，自动回滚
pub fn rollback_if_needed(app: &AppHandle) {
    let updates_dir = match get_frontend_updates_dir(app) {
        Ok(d) => d,
        Err(_) => return,
    };
    let active_version = match read_active_version(app) {
        Some(v) => v,
        None => return,
    };

    let manifest_path = match get_boot_manifest_path(app) {
        Ok(p) => p,
        Err(_) => return,
    };

    let manifest: serde_json::Map<String, serde_json::Value> = if manifest_path.exists() {
        std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    let entry = manifest.get(&active_version);
    let boot_count = entry
        .and_then(|v| v.get("boot_count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // 如果 active_version 存在但从未成功 boot（boot_count == 0），连续 3 次后回滚
    // 注意：这里需要跨启动统计。我们简化策略：
    // 如果 manifest 中该版本存在且 boot_count == 0，并且有一个 boot_attempt_count >= 3，则回滚。
    // 为了简化，我们增加 boot_attempt_count。
    let boot_attempt_count = entry
        .and_then(|v| v.get("boot_attempt_count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if boot_count == 0 && boot_attempt_count >= 3 {
        log::warn!(
            "[FrontendUpdate] Version {} failed to boot {} times, rolling back.",
            active_version,
            boot_attempt_count
        );
        let version_dir = updates_dir.join(&active_version);
        let _ = std::fs::remove_dir_all(&version_dir);
        let active_version_path = get_active_version_path(app).unwrap();
        let _ = std::fs::remove_file(&active_version_path);
        return;
    }

    // 增加 boot_attempt_count
    let mut manifest = manifest;
    let mut entry = manifest
        .get(&active_version)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = entry.as_object_mut().unwrap();
    obj.insert(
        "boot_attempt_count".to_string(),
        serde_json::json!(boot_attempt_count + 1),
    );
    manifest.insert(active_version.clone(), entry);
    let _ = std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    );
}

/// 启动时调用：若 APK 版本升级，清空旧的前端 OTA 包
pub fn clear_on_apk_upgrade(app: &AppHandle) {
    let apk_version = get_apk_version(app);
    let active_version = read_active_version(app);

    if let Some(active) = active_version {
        match (
            semver::Version::parse(&apk_version),
            semver::Version::parse(&active),
        ) {
            (Ok(apk), Ok(ota)) if apk > ota => {
                log::info!(
                    "[FrontendUpdate] APK upgraded from {} to {}, clearing old OTA packages.",
                    active,
                    apk_version
                );
                let _ = clear_frontend_updates_sync(app);
            }
            _ => {}
        }
    }
}

fn clear_frontend_updates_sync(app: &AppHandle) -> Result<(), String> {
    let updates_dir = get_frontend_updates_dir(app)?;
    if updates_dir.exists() {
        let _ = std::fs::remove_dir_all(&updates_dir);
    }
    Ok(())
}
