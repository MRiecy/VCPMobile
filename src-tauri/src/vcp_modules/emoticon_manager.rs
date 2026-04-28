use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use sqlx::Row;
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
    pub id: Option<i64>,
    pub category: String,
    pub filename: String,
    pub url: String,
    #[serde(rename = "searchKey")]
    pub search_key: String,
}

pub struct EmoticonManagerState {
    pub library: Arc<Mutex<Vec<EmoticonItem>>>,
}

impl Default for EmoticonManagerState {
    fn default() -> Self {
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

    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, item) in dp[0].iter_mut().enumerate() {
        *item = j;
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

#[derive(Deserialize)]
struct EmojiListResponse {
    data: std::collections::HashMap<String, Vec<String>>,
}

/// Internal (non-Tauri-command) version callable from lifecycle/bootstrap.
pub async fn refresh_emoticon_library_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<usize, String> {
    // 1. 获取配置
    let settings_state = app_handle.state::<SettingsState>();
    let settings = read_settings(app_handle.clone(), settings_state).await?;
    let vcp_server_url = settings.vcp_server_url;
    let admin_user = settings.admin_username;
    let admin_pass = settings.admin_password;
    let file_key = settings.file_key;

    if vcp_server_url.is_empty() {
        return Err("VCP Server URL is empty in settings".to_string());
    }
    if admin_user.is_empty() || admin_pass.is_empty() {
        return Err("管理员账号或密码未配置，请在 设置 → 用户档案 或 设置 → 数据同步 中填写管理员账号和密码".to_string());
    }
    if file_key.is_empty() {
        return Err("表情包图床密钥 (fileKey) 未配置，请在 设置 → 数据同步 中填写".to_string());
    }

    // 2. 构造基础 URL (去除末尾斜杠并规范化)
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

    // 3. 请求远程接口
    let client = reqwest::Client::new();
    let api_url = format!("{}/admin_api/emojis/list", base_url);

    let mut req = client.get(&api_url);
    if !admin_user.is_empty() && !admin_pass.is_empty() {
        req = req.basic_auth(&admin_user, Some(&admin_pass));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Failed to fetch emoji list: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("API {} — URL: {}", resp.status(), api_url));
    }

    let payload: EmojiListResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse emoji JSON: {}", e))?;

    // 4. 处理并保存到数据库
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;

    // 清空旧库
    sqlx::query("DELETE FROM emoticon_library")
        .execute(&mut *transaction)
        .await
        .map_err(|e| e.to_string())?;

    let mut library = Vec::new();
    for (category, filenames) in payload.data {
        for filename in filenames {
            let encoded_filename = utf8_percent_encode(&filename, VCP_ENCODE_SET).to_string();
            let encoded_category = utf8_percent_encode(&category, VCP_ENCODE_SET).to_string();

            // 构造完整 URL: baseUrl/pw=fileKey/images/category/filename
            let full_url = format!(
                "{}/pw={}/images/{}/{}",
                base_url, file_key, encoded_category, encoded_filename
            );

            let search_key = format!("{}/{}", category.to_lowercase(), filename.to_lowercase());

            // 插入数据库
            sqlx::query(
                "INSERT INTO emoticon_library (category, filename, url, search_key) VALUES (?, ?, ?, ?)"
            )
            .bind(&category)
            .bind(&filename)
            .bind(&full_url)
            .bind(&search_key)
            .execute(&mut *transaction)
            .await
            .map_err(|e| e.to_string())?;

            library.push(EmoticonItem {
                id: None,
                category: category.clone(),
                filename: filename.clone(),
                url: full_url,
                search_key,
            });
        }
    }

    transaction.commit().await.map_err(|e| e.to_string())?;

    let count = library.len();
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    *emoticon_state.library.lock().await = library;

    println!(
        "[EmoticonManager] Library regenerated via API: {} items",
        count
    );
    Ok(count)
}

#[tauri::command]
pub async fn regenerate_emoticon_library<R: Runtime>(
    app_handle: AppHandle<R>,
    _settings_state: State<'_, SettingsState>,
    _emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<usize, String> {
    refresh_emoticon_library_internal(&app_handle).await
}

pub async fn internal_load_library<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<Vec<EmoticonItem>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let rows = sqlx::query("SELECT id, category, filename, url, search_key FROM emoticon_library")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    let mut new_library = Vec::new();
    for row in rows {
        new_library.push(EmoticonItem {
            id: Some(row.get("id")),
            category: row.get("category"),
            filename: row.get("filename"),
            url: row.get("url"),
            search_key: row.get("search_key"),
        });
    }
    Ok(new_library)
}

#[tauri::command]
pub async fn get_emoticon_library<R: Runtime>(
    app_handle: AppHandle<R>,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<Vec<EmoticonItem>, String> {
    let mut library = emoticon_state.library.lock().await;
    if !library.is_empty() {
        return Ok(library.clone());
    }

    // 从数据库加载
    let new_library = internal_load_library(&app_handle).await?;
    *library = new_library.clone();
    Ok(new_library)
}

#[tauri::command]
pub async fn fix_emoticon_url<R: Runtime>(
    app_handle: AppHandle<R>,
    original_src: String,
    emoticon_state: State<'_, EmoticonManagerState>,
) -> Result<String, String> {
    let mut library_lock = emoticon_state.library.lock().await;

    // 如果内存为空，尝试从数据库加载一次
    if library_lock.is_empty() {
        if let Ok(new_library) = internal_load_library(&app_handle).await {
            *library_lock = new_library;
        }
    }

    let library = &*library_lock;
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
