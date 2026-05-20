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
      lowerName.includes("pseudo")
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
// @ts-expect-error process is a nodejs global
const detectedHost = process.env.TAURI_DEV_HOST;

// 如果 Tauri 自动识别的 host 不在真实的物理 IP 列表中（被 TUN 网卡劫持），则强制纠正为物理局域网 IP
const host = (detectedHost && physicalIps.includes(detectedHost))
  ? detectedHost
  : (physicalIps[0] || "0.0.0.0");

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    vue(),
    UnoCSS(),
  ],

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
}));
