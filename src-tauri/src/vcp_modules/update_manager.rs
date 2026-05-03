use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager};
use tokio::io::AsyncWriteExt;

const GITHUB_API_LATEST_URL: &str = "https://api.github.com/repos/MRiecy/VCPMobile/releases/latest";
const GITHUB_API_LIST_URL: &str =
    "https://api.github.com/repos/MRiecy/VCPMobile/releases?per_page=1";
const APK_ASSET_SUFFIX: &str = "arm64-v8a.apk";
const APK_FILENAME: &str = "update.apk";

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
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("获取缓存目录失败: {}", e))?;

    let apk_path = cache_dir.join(APK_FILENAME);

    // 如果存在旧文件，先删除
    if apk_path.exists() {
        let _ = tokio::fs::remove_file(&apk_path).await;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(&url)
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
    let mut file = tokio::fs::File::create(&apk_path)
        .await
        .map_err(|e| format!("创建文件失败: {}", e))?;

    use futures_util::StreamExt;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("下载流错误: {}", e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入文件失败: {}", e))?;
        downloaded += chunk.len() as u64;

        let _ = on_progress.send(DownloadProgress { downloaded, total });
    }

    file.flush().await.map_err(|e| e.to_string())?;

    // 校验文件大小是否与预期一致
    if let Some(expected) = total {
        if downloaded != expected {
            let _ = tokio::fs::remove_file(&apk_path).await;
            return Err("下载文件不完整，请重试".to_string());
        }
    }

    Ok(apk_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn install_update(app: AppHandle, apk_path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    // 尝试用 opener 打开本地 APK 触发系统安装器
    let result = app
        .opener()
        .open_path(&apk_path, Some("application/vnd.android.package-archive"));

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
