use crate::vcp_modules::chat_manager::ChatMessage;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 处理消息中的资产路径重定向 (Path Rebasing)
/// 处理跨端路径适配、缩略图路径校准以及头像路径的兼容性转换。
pub fn rebase_message_assets(
    app_handle: &AppHandle,
    item_id: &str,
    messages: &mut [ChatMessage],
) -> Result<(), String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let config_dir_str = config_dir.to_string_lossy().replace("\\", "/");

    for msg in messages {
        // 1. 修复附件路径 (Path Rebasing)
        if let Some(attachments) = &mut msg.attachments {
            for att in attachments {
                if let Some(hash) = &att.hash {
                    if let Some(real_path) =
                        crate::vcp_modules::file_manager::resolve_attachment_path(
                            app_handle, hash, &att.name,
                        )
                    {
                        let new_src = format!("file://{}", real_path.replace("\\", "/"));
                        if att.src != new_src {
                            println!("[VCPCore] Rebasing attachment: {} -> {}", att.src, new_src);
                            att.src = new_src;
                        }

                        // 同时校准缩略图路径
                        let thumb_path = PathBuf::from(&real_path);
                        if let Some(parent) = thumb_path.parent() {
                            let mut t = parent.to_path_buf();
                            t.push("thumbnails");
                            t.push(format!("{}_thumb.webp", hash));
                            if t.exists() {
                                att.thumbnail_path = Some(format!(
                                    "file://{}",
                                    t.to_string_lossy().replace("\\", "/")
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 2. 替换 extra 里的 avatarUrl (原有逻辑)
        if let Some(avatar_url) = msg.extra.get_mut("avatarUrl") {
            let mut new_url = None;
            if let Some(url_str_raw) = avatar_url.as_str() {
                // 1. 去除 file:// 前缀并统一斜杠
                let url_str = url_str_raw.trim_start_matches("file://").replace("\\", "/");

                // 2. 处理桌面端 Agents 路径
                if url_str.contains("AppData/Agents") {
                    let parts: Vec<&str> = url_str.split('/').collect();
                    if let Some(agent_idx) = parts.iter().position(|&r| r == "Agents") {
                        if parts.len() > agent_idx + 1 {
                            let relative_path = parts[agent_idx + 1..].join("/");
                            new_url = Some(format!("{}/agents/{}", config_dir_str, relative_path));
                        }
                    }
                }
                // 3. 处理桌面端 AgentGroups 路径
                else if url_str.contains("AppData/AgentGroups") {
                    let parts: Vec<&str> = url_str.split('/').collect();
                    if let Some(group_idx) = parts.iter().position(|&r| r == "AgentGroups") {
                        if parts.len() > group_idx + 1 {
                            let relative_path = parts[group_idx + 1..].join("/");
                            new_url =
                                Some(format!("{}/AgentGroups/{}", config_dir_str, relative_path));
                        }
                    }
                }
                // 4. 兼容旧版 VChat 格式: /chat_api/avatar/agent/...
                else if url_str.starts_with("/chat_api/") || url_str.starts_with("/avatar/") {
                    let mut found_path = None;
                    let extensions = ["png", "jpg", "jpeg", "webp", "gif"];

                    if let Some(agent_name) = &msg.name {
                        let mut avatarimage_dir = config_dir.clone();
                        avatarimage_dir.push("avatarimage");
                        for ext in extensions {
                            let possible_path =
                                avatarimage_dir.join(format!("{}.{}", agent_name, ext));
                            if possible_path.exists() {
                                found_path =
                                    Some(possible_path.to_string_lossy().replace("\\", "/"));
                                break;
                            }
                        }
                    }

                    if found_path.is_none() {
                        let mut agent_dir = config_dir.clone();
                        agent_dir.push("agents");
                        agent_dir.push(item_id);
                        for ext in extensions {
                            let possible_path = agent_dir.join(format!("avatar.{}", ext));
                            if possible_path.exists() {
                                found_path =
                                    Some(possible_path.to_string_lossy().replace("\\", "/"));
                                break;
                            }
                        }
                    }

                    if let Some(path) = found_path {
                        new_url = Some(path);
                    }
                }
            }
            if let Some(path) = new_url {
                *avatar_url = serde_json::Value::String(path);
            }
        }
    }

    Ok(())
}
