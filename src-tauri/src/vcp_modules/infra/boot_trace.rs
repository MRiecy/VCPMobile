//! 启动分段计时（BootTrace）。
//!
//! 冷启动各阶段的轻量时间标记：纯内存、无网络、无 DB。
//! setup/bootstrap 顺序写入，READY 后由前端一次性读取并与前端 marks 合并分析。
//! t0 为 `BootTraceState::new()`（即 setup 起点，近似进程起点）。

use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Clone, Serialize)]
pub struct BootStageMark {
    pub name: String,
    /// 自 setup 起点起的毫秒偏移
    #[serde(rename = "atMs")]
    pub at_ms: u64,
}

pub struct BootTraceState {
    t0: Instant,
    marks: Mutex<Vec<BootStageMark>>,
}

impl BootTraceState {
    pub fn new() -> Self {
        Self {
            t0: Instant::now(),
            marks: Mutex::new(Vec::with_capacity(32)),
        }
    }

    pub fn mark(&self, name: &str) {
        let at_ms = self.t0.elapsed().as_millis() as u64;
        if let Ok(mut marks) = self.marks.lock() {
            marks.push(BootStageMark {
                name: name.to_string(),
                at_ms,
            });
        }
    }

    pub fn snapshot(&self) -> Vec<BootStageMark> {
        self.marks.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

/// 顺序无关的安全打点：state 未注册时静默丢弃（setup 早期之外不应发生）。
pub fn boot_mark(app: &AppHandle, name: &str) {
    if let Some(state) = app.try_state::<BootTraceState>() {
        state.mark(name);
    }
}

#[tauri::command]
pub async fn get_boot_trace(
    state: State<'_, BootTraceState>,
) -> Result<Vec<BootStageMark>, String> {
    Ok(state.snapshot())
}

/// 前端合并后的完整启动轨迹，logcat/日志文件各留一份，便于 `pnpm android:debug:logs`
/// 与 run-as 拉取做冷启动 A/B 分析。
/// 常驻 Release 的代价被刻意压到极限：marks 只在启动链路各打一次，本命令每次冷启动
/// 仅追加一行 JSONL；文件保留最近 64 次启动记录，超出即截断，不会无限增长。
#[tauri::command]
pub async fn save_boot_trace(app: AppHandle, payload: String) -> Result<(), String> {
    log::info!("[BootTrace] {}", payload);

    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let file = dir.join("boot_trace.jsonl");
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .await
        .map_err(|e| e.to_string())?;
    use tokio::io::AsyncWriteExt;
    f.write_all(payload.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    f.write_all(b"\n").await.map_err(|e| e.to_string())?;
    drop(f);

    // 截断：只保留最近 64 次启动记录
    const MAX_LINES: usize = 64;
    if let Ok(content) = tokio::fs::read_to_string(&file).await {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > MAX_LINES {
            let kept = lines[lines.len() - MAX_LINES..].join("\n") + "\n";
            tokio::fs::write(&file, kept)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
