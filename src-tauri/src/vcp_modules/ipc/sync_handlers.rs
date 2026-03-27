use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use tauri::AppHandle;
use tauri::Manager;

#[derive(Serialize)]
pub struct LocalFileInfo {
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: u128,
    pub size: u64,
}

/// 将桌面端的相对路径映射为手机端的本地绝对路径
fn map_remote_path_to_local(app_handle: &AppHandle, remote_path: &str) -> Result<PathBuf, String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    let path_str = remote_path.replace("\\", "/");
    let parts: Vec<&str> = path_str.split('/').collect();

    if parts.is_empty() {
        return Err("Empty path".to_string());
    }

    let mut local_path = PathBuf::new();

    match parts[0].to_lowercase().as_str() {
        "agents" => {
            local_path.push(config_dir);
            local_path.push("Agents"); // 统一使用大写 A 对齐全局逻辑
            for part in &parts[1..] {
                local_path.push(part);
            }
        }
        "agentgroups" => {
            local_path.push(config_dir);
            local_path.push("AgentGroups"); // 明确支持群组目录同步
            for part in &parts[1..] {
                local_path.push(part);
            }
        }
        "userdata" => {
            local_path.push(config_dir);
            local_path.push("data"); // 桌面端的 UserData 对应手机端的 data 目录
            for part in &parts[1..] {
                local_path.push(part);
            }
        }
        "avatarimage" => {
            local_path.push(data_dir);
            local_path.push("avatarimage"); // 头像等媒体资源存放在 app_data_dir
            for part in &parts[1..] {
                local_path.push(part);
            }
        }
        "settings.json" => {
            local_path.push(config_dir);
            local_path.push("settings.json");
        }
        _ => {
            // 默认回退到 config_dir
            local_path.push(config_dir);
            for part in parts {
                local_path.push(part);
            }
        }
    }

    Ok(local_path)
}

#[tauri::command]
pub async fn sync_ping(url: String, token: String) -> Result<String, String> {
    println!("[VCPMobileSync] 发起 Ping 请求 -> {}", url);
    let client = Client::new();
    let res = client
        .get(&url)
        .header("x-sync-token", token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            println!("[VCPMobileSync] Ping 网络错误: {}", e);
            format!("Network error: {}", e)
        })?;

    if !res.status().is_success() {
        let status = res.status();
        println!("[VCPMobileSync] Ping 失败，HTTP 状态码: {}", status);
        return Err(format!("HTTP Error: {}", status));
    }

    let text = res
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    println!("[VCPMobileSync] Ping 成功，返回数据: {}", text);
    Ok(text)
}

#[tauri::command]
pub async fn sync_fetch_manifest(url: String, token: String) -> Result<String, String> {
    println!("[VCPMobileSync] 发起 Manifest 请求 -> {}", url);
    let client = Client::new();
    let res = client
        .get(&url)
        .header("x-sync-token", token)
        .header("Accept-Encoding", "gzip")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            println!("[VCPMobileSync] Manifest 网络错误: {}", e);
            format!("Network error: {}", e)
        })?;

    if !res.status().is_success() {
        let status = res.status();
        println!("[VCPMobileSync] Manifest 失败，HTTP 状态码: {}", status);
        return Err(format!("HTTP Error: {}", status));
    }

    let text = res
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    println!(
        "[VCPMobileSync] Manifest 获取成功，数据长度: {} bytes",
        text.len()
    );
    Ok(text)
}

#[tauri::command]
pub async fn sync_download_file(
    app_handle: AppHandle,
    url: String,
    token: String,
    relative_path: String,
) -> Result<(), String> {
    let local_path = map_remote_path_to_local(&app_handle, &relative_path)?;

    // 确保父目录存在
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    println!(
        "[VCPMobileSync] 开始下载文件 -> {} (保存至: {:?})",
        url, local_path
    );

    // 发起下载请求
    let client = Client::new();
    let response = client
        .get(&url)
        .header("x-sync-token", token)
        .send()
        .await
        .map_err(|e| {
            println!("[VCPMobileSync] 下载网络错误 ({}): {}", relative_path, e);
            format!("Request failed: {}", e)
        })?;

    if !response.status().is_success() {
        let status = response.status();
        println!(
            "[VCPMobileSync] 下载失败 ({})，HTTP 状态码: {}",
            relative_path, status
        );
        return Err(format!("Server returned error: {}", status));
    }

    // 读取二进制流
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes: {}", e))?;

    // 原子写入：先写入临时文件，再重命名
    let temp_path = local_path.with_extension("tmp_download");

    // 如果下载的是全局配置文件，进行智能合并，保留移动端专属的同步配置
    if relative_path == "settings.json" && local_path.exists() {
        if let Ok(local_content) = tokio::fs::read_to_string(&local_path).await {
            if let Ok(local_json) = serde_json::from_str::<serde_json::Value>(&local_content) {
                if let Ok(mut downloaded_json) = serde_json::from_slice::<serde_json::Value>(&bytes)
                {
                    // 保留移动端特有字段
                    let keys_to_preserve = vec!["syncServerIp", "syncServerPort", "syncToken"];
                    for key in keys_to_preserve {
                        if let Some(val) = local_json.get(key) {
                            downloaded_json[key] = val.clone();
                        }
                    }
                    // 重新写入合并后的内容到 temp_path
                    if let Ok(merged_bytes) = serde_json::to_vec_pretty(&downloaded_json) {
                        fs::write(&temp_path, merged_bytes)
                            .map_err(|e| format!("Failed to write merged file: {}", e))?;
                    } else {
                        fs::write(&temp_path, &bytes)
                            .map_err(|e| format!("Failed to write file: {}", e))?;
                    }
                } else {
                    fs::write(&temp_path, &bytes)
                        .map_err(|e| format!("Failed to write file: {}", e))?;
                }
            } else {
                fs::write(&temp_path, &bytes)
                    .map_err(|e| format!("Failed to write file: {}", e))?;
            }
        } else {
            fs::write(&temp_path, &bytes).map_err(|e| format!("Failed to write file: {}", e))?;
        }
    } else {
        fs::write(&temp_path, &bytes).map_err(|e| format!("Failed to write file: {}", e))?;
    }

    fs::rename(&temp_path, &local_path).map_err(|e| format!("Failed to rename file: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn sync_get_local_manifest(
    app_handle: AppHandle,
    paths: Vec<String>,
) -> Result<HashMap<String, LocalFileInfo>, String> {
    let mut manifest = HashMap::new();

    for relative_path in paths {
        if let Ok(local_path) = map_remote_path_to_local(&app_handle, &relative_path) {
            if local_path.exists() {
                if let Ok(metadata) = fs::metadata(&local_path) {
                    let mtime = metadata
                        .modified()
                        .unwrap_or(UNIX_EPOCH)
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();

                    manifest.insert(
                        relative_path,
                        LocalFileInfo {
                            mtime_ms: mtime,
                            size: metadata.len(),
                        },
                    );
                }
            }
        }
    }

    Ok(manifest)
}
