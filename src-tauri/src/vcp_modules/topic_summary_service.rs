use crate::vcp_modules::app_settings_manager::{read_app_settings, AppSettingsState};
use crate::vcp_modules::path_topology_service::resolve_history_path;
use reqwest::Client;
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

/// 话题总结的默认 Prompt
const DEFAULT_SUMMARY_PROMPT: &str = "请根据以上对话内容，仅返回一个简洁的话题标题。要求：1. 标题长度控制在10个汉字以内。2. 标题本身不能包含任何标点符号、数字编号 or 任何非标题文字。3. 直接给出标题文字，不要添加任何解释或前缀。";

/// 话题总结的默认模型
const DEFAULT_SUMMARY_MODEL: &str = "gemini-2.5-flash";

/// 话题总结的默认温度系数
const DEFAULT_SUMMARY_TEMPERATURE: f64 = 0.7;

/// AI 请求超时时间 (秒)
const AI_REQUEST_TIMEOUT_SECS: u64 = 30;

/// AI 请求最大 Token 数
const AI_MAX_TOKENS: u32 = 4000;

pub async fn summarize_topic<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, AppSettingsState>,
    item_id: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    let settings = read_app_settings(app_handle.clone(), settings_state).await?;
    if settings.vcp_server_url.is_empty() || settings.vcp_api_key.is_empty() {
        return Err("VCP settings are missing".to_string());
    }

    // 1. 获取历史记录 (最近4条)
    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);
    if !history_path.exists() {
        return Err("History not found".to_string());
    }

    let content = fs::read_to_string(&history_path).map_err(|e| e.to_string())?;
    let history: Vec<Value> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    if history.len() < 2 {
        return Err("Not enough messages to summarize".to_string());
    }

    let filtered_history: Vec<_> = history.iter().filter(|m| m["role"] != "system").collect();

    let recent_msgs: Vec<_> = filtered_history.iter().rev().take(4).rev().collect();

    let mut recent_content = String::new();
    for msg in recent_msgs {
        let role_name = if msg["role"] == "user" {
            settings.user_name.as_str()
        } else {
            agent_name.as_str()
        };
        let content_str = msg["content"].as_str().unwrap_or("");
        recent_content.push_str(&format!("{}: {}\n", role_name, content_str));
    }

    // 2. 构造 Prompt (对齐桌面端)
    let summary_prompt = format!(
        "[待总结聊天记录: {}]\n{}",
        recent_content, DEFAULT_SUMMARY_PROMPT
    );

    // 3. 调用 AI
    let client = Client::builder()
        .timeout(Duration::from_secs(AI_REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let model = settings
        .topic_summary_model
        .unwrap_or_else(|| DEFAULT_SUMMARY_MODEL.to_string());
    let temp = settings
        .topic_summary_model_temperature
        .unwrap_or(DEFAULT_SUMMARY_TEMPERATURE);

    let response = client
        .post(&settings.vcp_server_url)
        .header("Authorization", format!("Bearer {}", settings.vcp_api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "messages": [{"role": "user", "content": summary_prompt}],
            "model": model,
            "temperature": temp,
            "max_tokens": AI_MAX_TOKENS,
            "stream": false
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!("AI request failed: {}", response.status()));
    }

    let res_json: Value = response.json().await.map_err(|e| e.to_string())?;
    let raw_title = res_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();

    // 4. 清洗标题 (对齐桌面端 logic)
    let clean_title = clean_summarized_title(raw_title);

    if clean_title.is_empty() {
        return Err("AI failed to generate a valid title".to_string());
    }

    Ok(clean_title)
}

pub fn clean_summarized_title(raw: &str) -> String {
    // 提取第一行
    let first_line = raw.lines().next().unwrap_or("").trim();

    // 移除标点符号、前缀
    let mut cleaned = first_line
        .replace(|c: char| !c.is_alphanumeric() && !c.is_whitespace(), "")
        .replace("标题", "")
        .replace("总结", "")
        .replace("Topic", "")
        .replace(":", "")
        .replace("：", "")
        .trim()
        .to_string();

    // 移除所有空格
    cleaned = cleaned.replace(char::is_whitespace, "");

    // 截断到12个字符
    let char_count = cleaned.chars().count();
    if char_count > 12 {
        cleaned.chars().take(12).collect()
    } else {
        cleaned
    }
}
