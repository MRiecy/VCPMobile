use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::infra::vcp_http_endpoint::{
    append_url_segments, resolve_http_origin, HttpEndpointError,
};
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{sync::Arc, time::Duration};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use url::Url;

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

fn build_emoticon_image_url(
    origin: &Url,
    file_key: &str,
    category: &str,
    filename: &str,
) -> Result<Url, HttpEndpointError> {
    let mut url = origin.clone();
    // Express decodes this route parameter once; each original value must be
    // encoded as a path segment, including any literal percent signs in the Key.
    append_url_segments(
        &mut url,
        &[&format!("pw={file_key}"), "images", category, filename],
    )?;
    Ok(url)
}

/// Internal (non-Tauri-command) version callable from lifecycle/bootstrap.
pub async fn refresh_emoticon_library_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    force: bool,
) -> Result<usize, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 检查表情包库是否为空以及上一次同步时间
    let db_count = match sqlx::query("SELECT COUNT(*) as count FROM emoticon_library")
        .fetch_one(pool)
        .await
    {
        Ok(row) => {
            let count: i64 = row.get("count");
            count as usize
        }
        Err(_) => 0,
    };

    if !force && db_count > 0 {
        let last_sync_row =
            sqlx::query("SELECT value FROM settings WHERE key = 'emoticon_last_sync'")
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

        if let Some(row) = last_sync_row {
            let last_sync_str: String = row.get("value");
            if let Ok(last_sync) = last_sync_str.parse::<i64>() {
                let now = crate::vcp_modules::infra::utils::now_millis();
                // 3 days = 3 * 24 * 60 * 60 * 1000 = 259_200_000 ms
                if now - last_sync < 259_200_000 {
                    log::info!(
                        "[EmoticonManager] Emoticon library last synced at {} (less than 3 days ago). Skipping sync.",
                        last_sync
                    );
                    return Ok(db_count);
                }
            }
        }
    }

    // 1. 获取配置
    let settings_state = app_handle.state::<SettingsState>();
    let settings = read_config(app_handle.clone(), settings_state).await?;
    let vcp_server_url = &settings.server.url;
    let admin_user = &settings.server.admin_username;
    let admin_pass = &settings.server.admin_password;
    let file_key = &settings.server.file_key;

    // 表情服务固定部署在服务根地址，不继承聊天端点路径和查询参数。
    let origin = resolve_http_origin(vcp_server_url).map_err(|error| error.to_string())?;
    if admin_user.is_empty() || admin_pass.is_empty() {
        return Err(
            "管理员账号或密码未配置，请在 设置 → 用户身份 中填写管理员账号和密码".to_string(),
        );
    }
    if file_key.is_empty() {
        return Err("表情包图床密钥 (fileKey) 未配置，请在 设置 → 服务器连接 中填写".to_string());
    }

    // 2. 请求远程接口（共享 AdminApi 画像 Client，见 infra/http_clients.rs）
    let client = crate::vcp_modules::infra::http_clients::client(
        crate::vcp_modules::infra::http_clients::HttpProfile::AdminApi,
    );
    let mut api_url = origin.clone();
    append_url_segments(&mut api_url, &["admin_api", "emojis", "list"])
        .map_err(|error| error.to_string())?;

    // 包括等待响应头和完整响应体；失败发生在写库前，保留已有表情库。
    let mut req = client.get(api_url.clone()).timeout(Duration::from_secs(30));
    if !admin_user.is_empty() && !admin_pass.is_empty() {
        req = req.basic_auth(admin_user, Some(admin_pass));
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

    // 3. Build the new library before entering the write gate.
    let mut library = Vec::new();
    for (category, filenames) in payload.data {
        for filename in filenames {
            let full_url = build_emoticon_image_url(&origin, file_key, &category, &filename)
                .map_err(|error| error.to_string())?;

            let search_key = format!("{}/{}", category.to_lowercase(), filename.to_lowercase());

            library.push(EmoticonItem {
                id: None,
                category: category.clone(),
                filename,
                url: full_url.into(),
                search_key,
            });
        }
    }

    let mut transaction = db_state.write_transaction("emoticon.refresh").await?;
    sqlx::query("DELETE FROM emoticon_library")
        .execute(&mut *transaction)
        .await
        .map_err(|e| e.to_string())?;

    for item in &library {
        sqlx::query(
                "INSERT INTO emoticon_library (category, filename, url, search_key) VALUES (?, ?, ?, ?)"
            )
            .bind(&item.category)
            .bind(&item.filename)
            .bind(&item.url)
            .bind(&item.search_key)
            .execute(&mut *transaction)
            .await
            .map_err(|e| e.to_string())?;
    }

    // 保存最后同步时间戳
    let now = crate::vcp_modules::infra::utils::now_millis();
    sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('emoticon_last_sync', ?, ?)")
        .bind(now.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|e| e.to_string())?;

    transaction.commit().await.map_err(|e| e.to_string())?;

    let count = library.len();
    let emoticon_state = app_handle.state::<EmoticonManagerState>();
    *emoticon_state.library.lock().await = library;

    log::info!(
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
    refresh_emoticon_library_internal(&app_handle, true).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::settings_manager::{
        ConfigDomain, ConfigSaveResult, ConfigUpdate, DomainPatch,
    };
    use axum::{
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
        Json, Router,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn decoded_segments(url: &Url) -> Vec<String> {
        url.path_segments()
            .unwrap()
            .map(|segment| {
                percent_decode_str(segment)
                    .decode_utf8()
                    .unwrap()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn image_key_and_names_are_encoded_once_as_independent_segments() {
        let origin = resolve_http_origin("https://example.invalid/v1/chatvcp/completions").unwrap();
        let category = "角色 #?%25 表情包";
        let filename = "你好 #?%23.png";
        for key in ["a#b", "a?b", "a/b", "a%23b", "a%b", "中文 +/=", "a'b"] {
            let url = build_emoticon_image_url(&origin, key, category, filename).unwrap();
            assert!(url.path().starts_with("/pw="));
            assert!(url.query().is_none());
            assert!(url.fragment().is_none());
            assert_eq!(
                decoded_segments(&url),
                [
                    format!("pw={key}"),
                    "images".into(),
                    category.into(),
                    filename.into()
                ],
            );
        }
        let url = build_emoticon_image_url(&origin, "a%23b", "a%2Fb", "c%25.png").unwrap();
        assert_eq!(url.path(), "/pw=a%2523b/images/a%252Fb/c%2525.png");
    }

    #[test]
    fn emoticon_service_uses_origin_with_ports_and_ipv6() {
        for address in [
            "https://example.invalid:8443/v1/chat/completions?tenant=unused",
            "https://example.invalid:8443/v1/chatvcp/completions",
            "https://example.invalid:8443/raw/custom/endpoint",
            "https://example.invalid:8443/",
            "http://[2001:db8::1]:6005/v1/chatvcp/completions",
        ] {
            let original = Url::parse(address).unwrap();
            let origin = resolve_http_origin(address).unwrap();
            let image = build_emoticon_image_url(&origin, "key", "category", "image.png").unwrap();
            assert_eq!(image.origin(), original.origin());
            assert_eq!(image.path(), "/pw=key/images/category/image.png");
            assert!(image.query().is_none());
        }
    }

    struct FixtureServer(tokio::task::JoinHandle<()>);

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    #[tokio::test]
    async fn forced_refresh_persists_encoded_urls_and_failed_refresh_preserves_library() {
        let fail_listing = Arc::new(AtomicBool::new(false));
        let failures = fail_listing.clone();
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(4);
        let category = "分类 #%25 表情包";
        let filename = "图片 ?%23.png";
        let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
            let failures = failures.clone();
            let request_tx = request_tx.clone();
            async move {
                request_tx.send((uri.clone(), headers)).await.unwrap();
                if uri.path() == "/admin_api/emojis/list" {
                    if failures.load(Ordering::SeqCst) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    return Json(json!({ "data": { (category): [filename] } })).into_response();
                }
                // ImageServer receives one decoded Express route parameter for
                // pw=<Key>; decoding each wire segment once models that boundary.
                let segments: Vec<_> = uri
                    .path()
                    .split('/')
                    .skip(1)
                    .map(|segment| {
                        percent_decode_str(segment)
                            .decode_utf8()
                            .unwrap()
                            .into_owned()
                    })
                    .collect();
                Json(json!({ "segments": segments })).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = FixtureServer(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0103_config_domains.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let db = DbState::new(pool, std::path::PathBuf::new());
        let settings = SettingsState::new();
        let initial = settings.read_snapshot(&db).await.unwrap();
        let file_key = "密钥#?/%23+=";
        let changes = json!({
            "url": format!("http://{address}/raw/custom/chat?unused=query"),
            "endpointMode": "raw",
            "adminUsername": "admin",
            "adminPassword": "password",
            "fileKey": file_key,
        })
        .as_object()
        .unwrap()
        .clone();
        let old_server = serde_json::to_value(&initial.data.server).unwrap();
        let base_values = changes
            .keys()
            .map(|field| (field.clone(), old_server[field].clone()))
            .collect();
        let saved = settings
            .apply(
                &db,
                ConfigUpdate {
                    patches: vec![DomainPatch {
                        domain: ConfigDomain::Server,
                        expected_revision: initial.domains[&ConfigDomain::Server].revision.clone(),
                        changes,
                        base_values,
                    }],
                },
                |_, _| {},
            )
            .await
            .unwrap();
        assert!(matches!(saved, ConfigSaveResult::Saved { .. }));

        sqlx::query("INSERT INTO emoticon_library (category, filename, url, search_key) VALUES ('old', 'old.png', 'https://old.invalid/old.png', 'old')")
            .execute(&db.pool).await.unwrap();
        let now = crate::vcp_modules::infra::utils::now_millis();
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ('emoticon_last_sync', ?, ?)",
        )
        .bind(now.to_string())
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(db.clone());
        app.manage(settings);
        app.manage(EmoticonManagerState::default());

        assert_eq!(
            refresh_emoticon_library_internal(app.handle(), false)
                .await
                .unwrap(),
            1
        );
        assert!(
            request_rx.try_recv().is_err(),
            "fresh cache should avoid a request"
        );
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(5),
                refresh_emoticon_library_internal(app.handle(), true)
            )
            .await
            .unwrap()
            .unwrap(),
            1,
        );
        let (list_uri, list_headers) = request_rx.recv().await.unwrap();
        assert_eq!(list_uri.to_string(), "/admin_api/emojis/list");
        assert_eq!(list_headers["authorization"], "Basic YWRtaW46cGFzc3dvcmQ=");
        let persisted = internal_load_library(app.handle()).await.unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].category, category);
        assert_eq!(persisted[0].filename, filename);
        assert_ne!(persisted[0].url, "https://old.invalid/old.png");
        assert_eq!(
            app.state::<EmoticonManagerState>().library.lock().await[0].url,
            persisted[0].url,
        );

        let response: serde_json::Value = reqwest::Client::new()
            .get(&persisted[0].url)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            response["segments"],
            json!([format!("pw={file_key}"), "images", category, filename])
        );
        let (image_uri, _) = request_rx.recv().await.unwrap();
        assert!(image_uri.query().is_none());
        assert!(image_uri.path().starts_with("/pw="));

        let before_sync: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'emoticon_last_sync'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        fail_listing.store(true, Ordering::SeqCst);
        assert!(tokio::time::timeout(
            Duration::from_secs(5),
            refresh_emoticon_library_internal(app.handle(), true)
        )
        .await
        .unwrap()
        .is_err());
        assert_eq!(
            internal_load_library(app.handle()).await.unwrap()[0].url,
            persisted[0].url
        );
        assert_eq!(
            app.state::<EmoticonManagerState>().library.lock().await[0].url,
            persisted[0].url
        );
        let after_sync: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'emoticon_last_sync'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(after_sync, before_sync);
    }
}
