# PHZ110 真机证据索引

## 设备与构建

| 字段 | 值 |
|---|---|
| 设备 | OPPO PHZ110 |
| Android | 16 / API 36 |
| ABI | `arm64-v8a` |
| 显示 | 1080×2376；有效 density 480；竖屏 CSS viewport 360×792 |
| WebView | `com.google.android.webview` 150.0.7871.181 |
| 导航 | 手势导航 |
| 测试包 | `com.vcp.avatar.debug` `1.1.4-debug` |
| arm64 Debug APK SHA-256 | `1dc7542e323af8bb0c9cc773ded5b355a436d1a775bf0b870df37fd66f6d8847` |
| 前端加载方式 | `scripts/tauri_android_dev.cjs --usb`；ADB reverse 1420/1421；实时 Vite 工作树 |

`com.vcp.avatar` 是用户自用 Release。本轮没有启动、停止、清数据、卸载、安装或覆盖该包。

## 最小截图集

| 文件 | 证明范围 |
|---|---|
| `01-longpress-menu.png` | 深浅按钮长按后的气泡、统一、刊物快捷菜单 |
| `02-panel-chat.png` | 统一模式消息外壳 |
| `03-immersive-chat.png` | 刊物模式阅读栏与隐藏头像 |
| `04-landscape-theme-modes.png` | 横屏主题页中的主题轨道与三模式区域 |
| `05-font-150-fixed.png` | 360px 宽、字体 1.5× 修正后的横向短标签与可换行说明 |
| `06-final-restored.png` | 结束时恢复熊熊假日、深色、气泡基线 |

截图只证明可见几何状态。短按/长按互斥、跨重启持久化、滚动锚点约 0.33px 误差、自动旋转与字体恢复同时由 2026-08-13 的 ADB 触控与状态记录证明。

## 证据边界

本目录不证明平板、分屏/折叠、最低 WebView、输入法组合、全部强渲染 fixture 或主题草稿确认事务。不得将本机结果标记为完整 `DEVICE-VERIFIED`。
