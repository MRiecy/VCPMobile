use crate::vcp_modules::app_settings_manager::{read_app_settings, AppSettingsState};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use url::Url;

// 模拟 JS 的 encodeURIComponent 排除集
const VCP_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmoticonItem {
    pub url: String,
    pub category: String,
    pub filename: String,
    #[serde(rename = "searchKey")]
    pub search_key: String,
}

pub struct EmoticonManagerState {
    pub library: Arc<Mutex<Vec<EmoticonItem>>>,
}

impl EmoticonManagerState {
    pub fn new() -> Self {
        Self {
            library: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// 核心模糊匹配逻辑 (Levenshtein 算法)
fn edit_distance(s1: &str, s2: &str) -> usize {
    let s1_chars: Vec<char> = s1.chars().collect();
    let s2_chars: Vec<char> = s2.chars().collect();
    let n = s1_chars.len();
    let m = s2_chars.len();

    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut dp = vec![vec![0; m + 1]; n + 1];

    for i in 0..=n {
        dp[i][0] = i;
    }
    for j in 0..=m {
        dp[0][j] = j;
    }

    for i in 1..=n {
        for j in 1..=m {
            let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = std::cmp::min(
                std::cmp::min(dp[i - 1][j] + 1, dp[i][j - 1] + 1),
                dp[i - 1][j - 1] + cost,
            );
        }
    }
    dp[n][m]
}

fn get_similarity(s1: &str, s2: &str) -> f64 {
    let l1 = s1.chars().count();
    let l2 = s2.chars().count();
    let longer_length = std::cmp::max(l1, l2);
    if longer_length == 0 {
        return 1.0;
    }
    (longer_length - edit_distance(s1.to_lowercase().as_str(), s2.to_lowercase().as_str())) as f64
        / longer_length as f64
}

/// 提取 URL 信息 (对齐 JS: extractEmoticonInfo)
fn extract_emoticon_info(url_str: &str) -> (Option<String>, Option<String>) {
    // 1. 去掉协议和域名，只保留路径部分
    let path_part = if let Some(pos) = url_str.find("/images/") {
        &url_str[pos + 8..]
    } else if let Some(pos) = url_str.find("/pw=") {
        // 尝试跳过密码段直接找 images
        if let Some(img_pos) = url_str[pos..].find("/images/") {
            &url_str[pos + img_pos + 8..]
        } else {
            url_str
        }
    } else {
        url_str
    };

    // 2. 去除 Query 和 Fragment
    let clean_path = path_part
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");

    // 3. 解码并分割
    let decoded_path = percent_decode_str(clean_path).decode_utf8_lossy();
    let parts: Vec<&str> = decoded_path
        .split('/')
        .map(|s: &str| s)
        .filter(|s: &&str| !s.is_empty())
        .collect();

    if parts.len() >= 2 {
        let filename = parts.last().map(|s: &&str| s.to_string());
        let package_name = parts.get(parts.len() - 2).map(|s: &&str| s.to_string());
        (filename, package_name)
    } else if parts.len() == 1 {
        (Some(parts[0].to_string()), None)
    } else {
        (None, None)
    }
}

pub async fn internal_generate_library<R: Runtime>(
    app_handle: &AppHandle<R>,
    settings_state: &State<'_, AppSettingsState>,
) -> Result<Vec<EmoticonItem>, String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let generated_lists_path = config_dir.join("generated_lists");

    println!(
        "[EmoticonManager] Scanning path: {:?}",
        generated_lists_path
    );

    if !generated_lists_path.exists() {
        return Err("generated_lists directory not found".to_string());
    }

    // 1. 获取配置
    let settings = read_app_settings(app_handle.clone(), settings_state.clone()).await?;
    let vcp_server_url = settings.vcp_server_url;
    if vcp_server_url.is_empty() {
        return Err("VCP Server URL is empty in settings".to_string());
    }

    // 修复：保留端口号
    let base_url = if let Ok(u) = Url::parse(&vcp_server_url) {
        let scheme = u.scheme();
        let host = u.host_str().unwrap_or("");
        if let Some(port) = u.port() {
            format!("{}://{}:{}", scheme, host, port)
        } else {
            format!("{}://{}", scheme, host)
        }
    } else {
        return Err("Invalid VCP Server URL".to_string());
    };

    // 2. 获取密码 (鲁棒性改进)
    let config_env_path = generated_lists_path.join("config.env");
    if !config_env_path.exists() {
        return Err("config.env not found in generated_lists".to_string());
    }
    let config_content = fs::read_to_string(config_env_path).map_err(|e| e.to_string())?;
    let password = config_content
        .lines()
        .find(|line| line.trim().starts_with("file_key="))
        .and_then(|line| line.split('=').nth(1))
        .ok_or_else(|| "file_key not found in config.env".to_string())?
        .trim();

    println!("[EmoticonManager] Password loaded, BaseURL: {}", base_url);

    // 3. 扫描 txt 文件
    let mut library = Vec::new();
    let entries = fs::read_dir(&generated_lists_path).map_err(|e| e.to_string())?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.ends_with("表情包.txt") {
            let category = file_name.trim_end_matches(".txt").to_string();
            let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let filenames: Vec<&str> = content
                .split('|')
                .filter(|s| !s.trim().is_empty())
                .collect();

            println!(
                "[EmoticonManager] Loading category: {} ({} items)",
                category,
                filenames.len()
            );

            for filename in filenames {
                // 使用 VCP_ENCODE_SET 以保留点号等字符
                let encoded_filename = utf8_percent_encode(filename, VCP_ENCODE_SET).to_string();
                let encoded_category = utf8_percent_encode(&category, VCP_ENCODE_SET).to_string();
                let full_url = format!(
                    "{}/pw={}/images/{}/{}",
                    base_url, password, encoded_category, encoded_filename
                );

                library.push(EmoticonItem {
                    url: full_url,
                    category: category.clone(),
                    filename: filename.to_string(),
                    search_key: format!("{}/{}", category.to_lowercase(), filename.to_lowercase()),
                });
            }
        }
    }

    println!("[EmoticonManager] Total items: {}", library.len());

    // 保存到本地 json 缓存
    let library_json_path = generated_lists_path.join("emoticon_library.json");
    let json_content = serde_json::to_string_pretty(&library).map_err(|e| e.to_string())?;
    let _ = fs::write(library_json_path, json_content);

    Ok(library)
}

#[tauri::command]
pub async fn get_emoticon_library(
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<Vec<EmoticonItem>, String> {
    let library = emoticon_state.library.lock().await;
    Ok(library.clone())
}

#[tauri::command]
pub async fn regenerate_emoticon_library<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, AppSettingsState>,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<usize, String> {
    let library = internal_generate_library(&app_handle, &settings_state).await?;
    let count = library.len();
    *emoticon_state.library.lock().await = library;
    Ok(count)
}

#[tauri::command]
pub async fn fix_emoticon_url(
    original_src: String,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<String, String> {
    let library = emoticon_state.library.lock().await;
    if library.is_empty() {
        return Ok(original_src);
    }

    // 1. 完全匹配检查 (对齐 JS)
    // 注意：JS 版用了 decodeURIComponent，这里也尽量对齐
    let decoded_original = percent_decode_str(&original_src).decode_utf8_lossy();
    if library.iter().any(|item| {
        let decoded_item = percent_decode_str(&item.url).decode_utf8_lossy();
        decoded_item == decoded_original
    }) {
        return Ok(original_src);
    }

    // 2. 关键词检查
    if !decoded_original.contains("表情包") {
        return Ok(original_src);
    }

    // 3. 提取信息
    let (search_filename, search_package) = extract_emoticon_info(&original_src);
    let search_filename = match search_filename {
        Some(f) => f,
        None => return Ok(original_src),
    };

    let mut best_match: Option<&EmoticonItem> = None;
    let mut highest_score = -1.0;

    for item in library.iter() {
        // 计算包名相似度
        let package_score = if let Some(sp) = &search_package {
            get_similarity(sp, &item.category)
        } else if search_package.is_none() && item.category.is_empty() {
            1.0
        } else {
            0.5 // 对齐 JS 逻辑
        };

        // 计算文件名相似度
        let filename_score = get_similarity(&search_filename, &item.filename);

        // 加权评分: 70% 包名, 30% 文件名
        let score = (0.7 * package_score) + (0.3 * filename_score);

        if score > highest_score {
            highest_score = score;
            best_match = Some(item);
        }
    }

    // 4. 阈值判断 (对齐 JS: 0.6)
    if let Some(item) = best_match {
        if highest_score > 0.6 {
            // println!("[EmoticonManager] Fixed: {} -> {} (Score: {:.2})", original_src, item.url, highest_score);
            return Ok(item.url.clone());
        }
    }

    Ok(original_src)
}

/// 内部同步修复函数，用于消息处理器
pub fn internal_fix_url(original_src: &str, library: &[EmoticonItem]) -> String {
    if library.is_empty() {
        return original_src.to_string();
    }

    let decoded_original = percent_decode_str(original_src).decode_utf8_lossy();
    if library.iter().any(|item| {
        let decoded_item = percent_decode_str(&item.url).decode_utf8_lossy();
        decoded_item == decoded_original
    }) {
        return original_src.to_string();
    }

    if !decoded_original.contains("表情包") {
        return original_src.to_string();
    }

    let (search_filename, search_package) = extract_emoticon_info(original_src);
    let search_filename = match search_filename {
        Some(f) => f,
        None => return original_src.to_string(),
    };

    let mut best_match: Option<&EmoticonItem> = None;
    let mut highest_score = -1.0;

    for item in library.iter() {
        let package_score = if let Some(sp) = &search_package {
            get_similarity(sp, &item.category)
        } else if search_package.is_none() && item.category.is_empty() {
            1.0
        } else {
            0.5
        };

        let filename_score = get_similarity(&search_filename, &item.filename);
        let score = (0.7 * package_score) + (0.3 * filename_score);

        if score > highest_score {
            highest_score = score;
            best_match = Some(item);
        }
    }

    if let Some(item) = best_match {
        if highest_score > 0.6 {
            return item.url.clone();
        }
    }

    original_src.to_string()
}
