fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 按编译目标判断，避免在 Windows 上构建 Android 时传入 MSVC 链接参数。
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut attributes = tauri_build::Attributes::new();
    if windows_msvc {
        // Windows 仅用于本地开发/测试。由链接器统一为 bins 和 lib 测试嵌入
        // Common Controls v6 manifest；禁用 Tauri 的同编号资源，避免重复嵌入。
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    tauri_build::try_build(attributes)?;

    if windows_msvc {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
    Ok(())
}
