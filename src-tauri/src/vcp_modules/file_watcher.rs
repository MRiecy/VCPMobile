use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::index_service::index_history_file;
use notify_debouncer_full::{
    new_debouncer,
    notify::{self, *},
    Debouncer, FileIdMap,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// 文件监听状态，用于处理回环过滤
/// 对齐 @/plans/Rust文件数据管理重构详细规划.md 中的 2.2 节
pub struct WatcherState {
    /// 记录最后一次应用内部写入的时间戳 (毫秒)
    pub last_internal_save_time: AtomicU64,
}

impl Default for WatcherState {
    fn default() -> Self {
        Self {
            last_internal_save_time: AtomicU64::new(0),
        }
    }
}

/// 封装 Debouncer 资源，用于在 Tauri 状态中持有监听器
pub struct DebouncerResource {
    #[allow(dead_code)]
    pub debouncer: std::sync::Mutex<Debouncer<RecommendedWatcher, FileIdMap>>,
}

/// 信号：标记一次内部保存操作
/// 当应用自身修改文件时调用此命令，以避免触发 Watcher 的响应循环
#[tauri::command]
pub fn signal_internal_save(state: tauri::State<'_, WatcherState>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    state.last_internal_save_time.store(now, Ordering::SeqCst);
    println!("[Watcher] Internal save signaled at {}", now);
}

/// 初始化文件监听服务
/// 监听 AppData 下的 UserData/data 目录，实现外部变更的实时同步
pub fn init_watcher(app_handle: AppHandle) -> std::result::Result<(), String> {
    // 1. 获取并准备监听目录 (兼容 UserData/data)
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;

    let search_dirs = [config_dir.join("UserData"), config_dir.join("data")];

    // 2. 创建异步通道用于接收文件事件
    let (tx, rx) = std::sync::mpsc::channel();

    // 3. 初始化防抖监听器 (Debouncer)
    // 设置 500ms 的稳定窗口，确保像 Syncthing 这样的大文件写入完成后再处理
    let mut debouncer = new_debouncer(Duration::from_millis(500), None, tx)
        .map_err(|e: notify::Error| e.to_string())?;

    // 递归监听 UserData 和 data 目录
    for dir in &search_dirs {
        if dir.exists() {
            debouncer
                .watcher()
                .watch(dir, RecursiveMode::Recursive)
                .map_err(|e: notify::Error| e.to_string())?;
            println!("[Watcher] Now watching: {:?}", dir);
        }
    }

    let handle_clone = app_handle.clone();
    let config_dir_clone = config_dir.clone();
    let index_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(5));

    // 4. 开启后台线程处理事件流
    std::thread::spawn(move || {
        while let Ok(res) = rx.recv() {
            match res {
                Ok(events) => {
                    let state = handle_clone.state::<WatcherState>();
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    let last_save = state.last_internal_save_time.load(Ordering::SeqCst);

                    // 环路过滤逻辑：如果当前时间距离上次内部保存小于 2000ms，则视为内部改动，忽略之
                    if now - last_save < 2000 {
                        println!("[Watcher] Ignored internal save (within 2s window)");
                        continue;
                    }

                    for event in events {
                        println!("[Watcher] External change: {:?}", event.paths);

                        // a. 通知前端 UI：文件已变动，请触发 Delta Sync
                        let _ = handle_clone.emit("vcp-file-change", &event.paths);

                        // b. 如果变动的是聊天记录 (history.json)，触发影子数据库重索引
                        if let Some(db_state) = handle_clone.try_state::<DbState>() {
                            for path in &event.paths {
                                if path.file_name().and_then(|n| n.to_str()) == Some("history.json")
                                {
                                    let pool = db_state.pool.clone();
                                    let path_buf = path.to_path_buf();
                                    let sem = index_semaphore.clone();
                                    let app_config_dir = config_dir_clone.clone();
                                    // 异步执行索引更新，不阻塞事件循环
                                    tauri::async_runtime::spawn(async move {
                                        let _permit = sem
                                            .acquire()
                                            .await
                                            .unwrap_or_else(|e| panic!("Semaphore closed: {}", e));
                                        let _ =
                                            index_history_file(&app_config_dir, &path_buf, &pool)
                                                .await;
                                    });
                                }
                            }
                        }
                    }
                }
                Err(e) => println!("[Watcher] Error: {:?}", e),
            }
        }
    });

    // 5. 将监听器实例存入 Tauri 状态，防止被销毁
    app_handle.manage(DebouncerResource {
        debouncer: std::sync::Mutex::new(debouncer),
    });

    Ok(())
}
