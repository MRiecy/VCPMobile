use crate::vcp_modules::chat_manager::ChatMessage;
use std::fs;
use std::path::Path;

/// 纯粹的文件 IO 接口，负责读取和写入 history.json 原始数据。
/// 不包含路径修复或业务逻辑。
pub fn read_history(path: &Path) -> Result<Vec<ChatMessage>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let history: Vec<ChatMessage> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(history)
}

pub fn write_history(path: &Path, history: &[ChatMessage]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let content = serde_json::to_string_pretty(history).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/*
/// 追加单条消息到历史文件。
/// 注意：此函数不处理锁，锁逻辑应在业务层 (chat_manager) 处理。
pub fn append_history(path: &Path, message: ChatMessage) -> Result<(), String> {
    let mut history = read_history(path)?;

    // 检查是否已存在 (防止重复追加)
    if history.iter().any(|m| m.id == message.id) {
        return Ok(());
    }

    history.push(message);
    write_history(path, &history)
}
*/
