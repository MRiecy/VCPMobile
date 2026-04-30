use crate::vcp_modules::message_service;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::vcp_client::normalize_vcp_url;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, State};

/// 话题总结的默认 Prompt
const DEFAULT_SUMMARY_PROMPT: &str = "请根据以上对话内容，仅返回一个简洁的话题标题。要求：1. 标题长度控制在10个汉字以内。2. 标题本身不能包含任何标点符号、数字编号 or 任何非标题文字。3. 直接给出标题文字，不要添加任何解释或前缀。";

/// 话题总结的默认模型
const DEFAULT_SUMMARY_MODEL: &str = "gemini-2.5-flash";

/// 话题总结的默认温度系数 (硬编码为 0.7)
const DEFAULT_SUMMARY_TEMPERATURE: f64 = 0.7;

/// AI 请求超时时间 (秒)
const AI_REQUEST_TIMEOUT_SECS: u64 = 30;

/// AI 请求最大 Token 数
const AI_MAX_TOKENS: u32 = 4000;

pub async fn summarize_topic(
    app_handle: AppHandle,
    settings_state: State<'_, SettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    let settings = read_settings(app_handle.clone(), settings_state).await?;
    if settings.vcp_server_url.is_empty() || settings.vcp_api_key.is_empty() {
        return Err("VCP settings are missing".to_string());
    }

    // 1. 获取最近消息 (最近4条)
    let messages = message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        Some(4),
        None,
    )
    .await?;

    if messages.len() < 2 {
        return Err("Not enough messages to summarize".to_string());
    }

    let mut recent_content = String::new();
    for msg in messages {
        let role_name = if msg.role == "user" {
            settings.user_name.as_str()
        } else {
            agent_name.as_str()
        };
        recent_content.push_str(&format!("{}: {}\n", role_name, msg.content));
    }

    // 2. 构造 Prompt
    let summary_prompt = format!(
        "[待总结聊天记录: {}]\n{}",
        recent_content, DEFAULT_SUMMARY_PROMPT
    );

    // 3. 调用 AI
    let client = Client::builder()
        .timeout(Duration::from_secs(AI_REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let model = if settings.topic_summary_model.is_empty() {
        DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        settings.topic_summary_model
    };

    let vcp_url = normalize_vcp_url(&settings.vcp_server_url);
    let response = client
        .post(&vcp_url)
        .header("Authorization", format!("Bearer {}", settings.vcp_api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "messages": [{"role": "user", "content": summary_prompt}],
            "model": model,
            "temperature": DEFAULT_SUMMARY_TEMPERATURE,
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

    // 4. 清洗标题
    let clean_title = clean_summarized_title(raw_title);

    if clean_title.is_empty() {
        return Err("AI failed to generate a valid title".to_string());
    }

    Ok(clean_title)
}

pub fn clean_summarized_title(raw: &str) -> String {
    let first_line = raw.lines().next().unwrap_or("").trim();

    let mut cleaned = first_line
        .replace(|c: char| !c.is_alphanumeric() && !c.is_whitespace(), "")
        .replace("标题", "")
        .replace("总结", "")
        .replace("Topic", "")
        .replace(":", "")
        .replace("：", "")
        .trim()
        .to_string();

    cleaned = cleaned.replace(char::is_whitespace, "");

    let char_count = cleaned.chars().count();
    if char_count > 12 {
        cleaned.chars().take(12).collect()
    } else {
        cleaned
    }
}
