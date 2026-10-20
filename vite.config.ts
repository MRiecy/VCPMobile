import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import UnoCSS from "unocss/vite";

import os from "os";

// 智能探测并提取所有真实的物理局域网 IP，自动过滤 TUN/TAP 等代理虚拟网卡
function getPhysicalIps() {
  const interfaces = os.networkInterfaces();
  const physicalIps: string[] = [];
  for (const name of Object.keys(interfaces)) {
    const lowerName = name.toLowerCase();
    // 过滤掉所有虚拟网卡、VPN网卡和本地回环网卡
    if (
      lowerName.includes("tun") ||
      lowerName.includes("tap") ||
      lowerName.includes("clash") ||
      lowerName.includes("sing-box") ||
      lowerName.includes("wintun") ||
      lowerName.includes("vpn") ||
      lowerName.includes("virtual") ||
      lowerName.includes("vbox") ||
      lowerName.includes("vmware") ||
      lowerName.includes("loopback") ||
      lowerName.includes("pseudo") ||
      lowerName.includes("vethernet") ||
      lowerName.includes("wsl") ||
      lowerName.includes("hyper-v") ||
      lowerName.includes("host-only")
    ) {
      continue;
    }
    const ifaceList = interfaces[name] || [];
    for (const iface of ifaceList) {
      if (iface.family === "IPv4" && !iface.internal) {
        // 优先选用物理无线网卡 (Wi-Fi/WLAN) 或有线网卡 (Ethernet)
        if (
          lowerName.includes("wlan") ||
          lowerName.includes("wi-fi") ||
          lowerName.includes("ethernet") ||
          lowerName.includes("本地连接")
        ) {
          physicalIps.unshift(iface.address);
        } else {
          physicalIps.push(iface.address);
        }
      }
    }
  }
  return physicalIps;
}

const physicalIps = getPhysicalIps();
const detectedHost = process.env.TAURI_DEV_HOST;

// USB 模式（localhost）直接信任；WiFi 模式校验是否在物理 IP 列表中，防止 TUN 劫持
const isLocalhost = detectedHost === "localhost" || (detectedHost && detectedHost.startsWith("127."));
const host = isLocalhost
  ? detectedHost
  : (detectedHost && physicalIps.includes(detectedHost))
    ? detectedHost
    : (physicalIps[0] || "0.0.0.0");

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    vue(),
    UnoCSS(),
  ],

  // 本地 Tauri 插件会随源码同步增加 Guest JS 导出。排除依赖预打包，避免
  // node_modules/.vite 继续提供缺少新导出的旧快照，导致异步页面在首次加载时失败。
  optimizeDeps: {
    exclude: ["tauri-plugin-vcp-mobile"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    host: host || '0.0.0.0',
    strictPort: true,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // Shared mobile bundle: Android WebView and the minimum iOS WKWebView.
    // Syntax targets do not replace runtime API or native interaction checks.
    target: ["chrome117", "safari26.0"],
    cssTarget: ["chrome117", "safari26.0"],
    // Mermaid 图表与 ELK 布局引擎按需加载，原始体积偏大但不进入首屏；其余常驻依赖显式拆包，
    // 让主应用 chunk 保持在配置的 700KB 门槛以内。
    chunkSizeWarningLimit: 700,
    rolldownOptions: {
      input: {
        main: "index.html",
      },
      treeshake: {
        manualPureFunctions: process.env.NODE_ENV === "production"
          ? ["console.log", "console.debug", "console.info"]
          : [],
      },
      output: {
        codeSplitting: {
          groups: [
            {
              name: "vue-vendor",
              test: /[\\/]node_modules[\\/](?:@vue[\\/]|(?:vue|pinia|pinia-plugin-persistedstate|vue-router)[\\/])/,
            },
            {
              name: "render-vendor",
              test: /[\\/]node_modules[\\/](?:marked|morphdom|dompurify)[\\/]/,
            },
            {
              name: "tauri-vendor",
              test: /[\\/]node_modules[\\/]@tauri-apps[\\/]/,
            },
          ],
        },
      },
    },
  },
}));
