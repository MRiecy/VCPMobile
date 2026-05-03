use std::borrow::Cow;
use std::path::PathBuf;
use tauri::{Assets, Runtime};
use tauri_utils::assets::{AssetKey, AssetsIter, CspHash};

/// =================================================================
/// vcp_modules/ota_assets.rs - 前端资源 OTA 代理层
/// =================================================================
///
/// 通过实现 `tauri::Assets` trait，在 Tauri 服务前端资源时优先从
/// 文件系统（`app_config_dir/frontend_updates/<version>/`）读取更新后的
/// 资源，找不到时 fallback 到 APK 内置的 `EmbeddedAssets`。
///
/// 这保证了 WebView 始终停留在 `https://tauri.localhost/` origin，
/// IPC（invoke）完全不受影响，前端代码无需任何感知。
pub struct OtaAssets<R: Runtime> {
    embedded: Box<dyn Assets<R>>,
    update_dir: PathBuf,
}

impl<R: Runtime> OtaAssets<R> {
    pub fn new(embedded: Box<dyn Assets<R>>, update_dir: PathBuf) -> Self {
        Self {
            embedded,
            update_dir,
        }
    }
}

impl<R: Runtime> Assets<R> for OtaAssets<R> {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        if !self.update_dir.as_os_str().is_empty() {
            let fs_path = self.update_dir.join(key.as_ref().trim_start_matches('/'));
            if fs_path.is_file() {
                if let Ok(bytes) = std::fs::read(&fs_path) {
                    return Some(Cow::Owned(bytes));
                }
            }
        }
        self.embedded.get(key)
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        self.embedded.iter()
    }

    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        self.embedded.csp_hashes(html_path)
    }
}

/// 空 Assets 实现，仅用于 `set_assets` 时临时占位以取出旧的 embedded。
pub struct EmptyAssets;

impl<R: Runtime> Assets<R> for EmptyAssets {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}
