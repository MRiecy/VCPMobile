use super::PortalState;
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use uuid::Uuid;

#[tauri::command]
pub async fn open_native_portal<R: Runtime>(
    app_handle: AppHandle<R>,
    portal_state: State<'_, PortalState>,
    html: String,
) -> Result<(), String> {
    // 1. 构造唯一窗口标签和 ID
    let id = Uuid::new_v4().to_string();
    let label = format!("vcp-portal-{}", id);

    // 2. 存入状态
    portal_state.contents.insert(id.clone(), html);

    // 3. 构建 URL (使用私有协议绕过跨域和权限限制)
    let url = format!("vcp-portal://render?id={}", id);

    // 4. 创建原生窗口 (在移动端会自动覆盖全屏)
    let _window = WebviewWindowBuilder::new(
        &app_handle,
        &label,
        WebviewUrl::External(url.parse().map_err(|e| format!("Invalid URL: {}", e))?),
    )
    .build()
    .map_err(|e| format!("Failed to create native webview: {}", e))?;

    println!("[VcpHost] Native portal opened via protocol: {}", url);
    Ok(())
}

#[tauri::command]
pub fn close_native_portal<R: Runtime>(
    app_handle: AppHandle<R>,
    window: WebviewWindow<R>,
) -> Result<(), String> {
    // 遵循最终建议：使用 close() 替代 destroy()
    // 配合 AppHandle 模式确保操作的健壮性
    let label = window.label();
    if let Some(portal_window) = app_handle.get_webview_window(label) {
        portal_window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}
