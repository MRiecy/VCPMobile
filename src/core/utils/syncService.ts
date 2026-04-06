import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "../stores/settings";

export interface SyncTreeNode {
  name: string;
  path: string;
  type: "directory" | "file";
  sizeBytes: number;
  children?: Record<string, SyncTreeNode>;
}

export interface SyncManifest {
  [relativePath: string]: {
    mtimeMs: number;
    size: number;
    hash?: string;
  };
}

export interface ManifestResponse {
  status: string;
  tree: SyncTreeNode;
  manifest: SyncManifest;
}

/**
 * Promise 超时竞态工具
 */
const withTimeout = <T>(
  promise: Promise<T>,
  ms: number,
  errMsg = "请求超时",
): Promise<T> => {
  let timeoutId: any;
  const timeoutPromise = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(errMsg)), ms);
  });
  return Promise.race([promise, timeoutPromise]).finally(() =>
    clearTimeout(timeoutId),
  );
};

export class SyncService {
  private getBaseUrl(customIp?: string, customPort?: number): string {
    const settingsStore = useSettingsStore();
    const ip = customIp || settingsStore.settings?.syncServerIp || "127.0.0.1";
    const port = customPort || settingsStore.settings?.syncServerPort || 5974;
    return `http://${ip}:${port}/api/mobile-sync`;
  }

  /**
   * 获取本地文件清单（用于增量对比）
   */
  async getLocalManifest(paths: string[]): Promise<SyncManifest> {
    try {
      return await invoke<SyncManifest>("sync_get_local_manifest", { paths });
    } catch (e: any) {
      console.error("[SyncService] Failed to get local manifest:", e);
      return {};
    }
  }

  /**
   * 测试与桌面端同步服务器的连接
   *
   */
  async pingServer(
    customIp?: string,
    customPort?: number,
    customToken?: string,
  ): Promise<{ status: string; deviceName: string; message: string }> {
    const url = `${this.getBaseUrl(customIp, customPort)}/ping`;
    const settingsStore = useSettingsStore();
    const token =
      customToken !== undefined
        ? customToken
        : settingsStore.settings?.syncToken || "";

    try {
      const responseText = await invoke<string>("sync_ping", { url, token });
      return JSON.parse(responseText);
    } catch (e: any) {
      throw new Error(e.toString());
    }
  }

  /**
   * 获取同步清单和业务摘要
   * @param paths 可选的路径前缀过滤（英文逗号分隔）
   */
  async fetchManifest(paths?: string): Promise<ManifestResponse> {
    let url = `${this.getBaseUrl()}/manifest`;
    if (paths) {
      url += `?paths=${encodeURIComponent(paths)}`;
    }

    const settingsStore = useSettingsStore();
    const token = settingsStore.settings?.syncToken || "";

    try {
      const responseText = await invoke<string>("sync_fetch_manifest", {
        url,
        token,
      });
      return JSON.parse(responseText);
    } catch (e: any) {
      throw new Error(e.toString());
    }
  }

  /**
   * 下载单个文件并保存到本地
   * 注意：这里调用 Rust 后端命令以获得更好的性能和文件系统访问权限
   * @param relativePath 相对 AppData 的路径
   */
  async downloadFile(relativePath: string): Promise<void> {
    const url = `${this.getBaseUrl()}/download?path=${encodeURIComponent(relativePath)}`;
    const settingsStore = useSettingsStore();
    const token = settingsStore.settings?.syncToken || "";

    // 调用 Rust 命令进行下载和保存 (增加 15s 超时熔断)
    await withTimeout(
      invoke("sync_download_file", {
        url,
        token,
        relativePath,
      }),
      15000,
      `下载文件 ${relativePath} 超时`,
    );
  }

  /**
   * 核心同步引擎：顺序下载给定的文件列表
   * @param filesToDownload 需要下载的文件相对路径数组
   * @param onProgress 进度回调函数
   */
  async startSync(
    filesToDownload: string[],
    onProgress: (current: number, total: number, currentFile: string) => void,
  ): Promise<void> {
    if (filesToDownload.length === 0) return;

    const total = filesToDownload.length;

    if (total === 0) {
      onProgress(0, 0, "没有需要同步的文件");
      return;
    }

    // 2. 顺序下载文件 (避免手机端并发过高导致 OOM 或网络阻塞)
    let current = 0;
    for (const file of filesToDownload) {
      onProgress(current, total, file);
      try {
        await this.downloadFile(file);
      } catch (e) {
        console.error(`[SyncEngine] Failed to download ${file}:`, e);
        // 即使单个文件失败，也继续下载下一个
      }
      current++;
    }

    // 3. 重建数据库索引
    onProgress(total, total, "正在同步数据库记录...");
    // The DB is now the single source of truth; resync is no longer needed.

    // 4. 完成
    onProgress(total, total, "同步完成");

    // 5. 自动启动同步守护进程 (实时订阅)
    await this.startSyncDaemon().catch((e) =>
      console.error("[SyncService] Failed to start daemon:", e),
    );
  }

  /**
   * 启动实时同步守护进程 (WebSocket)
   */
  async startSyncDaemon(): Promise<void> {
    const settingsStore = useSettingsStore();
    const ip = settingsStore.settings?.syncServerIp || "127.0.0.1";
    // 注意：WS 端口是主端口 + 1
    const port = (settingsStore.settings?.syncServerPort || 5974) + 1;
    const token = encodeURIComponent(settingsStore.settings?.syncToken || "");

    const wsUrl = `ws://${ip}:${port}/?token=${token}`;
    console.log("[SyncService] Starting sync daemon:", wsUrl);

    return await invoke("start_sync_daemon", { wsUrl });
  }

  /**
   * 将本地话题全量推送到桌面端 (上行同步)
   */
  async pushTopicToDesktop(
    agentId: string,
    topicId: string,
    history: any[],
  ): Promise<void> {
    const url = `${this.getBaseUrl()}/upload-topic`;
    const settingsStore = useSettingsStore();
    const token = settingsStore.settings?.syncToken || "";

    try {
      const response = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-sync-token": token,
        },
        body: JSON.stringify({
          agentId,
          topicId,
          history,
        }),
      });

      if (!response.ok) {
        throw new Error(`Upload failed: ${response.statusText}`);
      }

      console.log(
        `[SyncService] Topic ${topicId} pushed to desktop successfully.`,
      );
    } catch (e) {
      console.error(
        `[SyncService] Failed to push topic ${topicId} to desktop:`,
        e,
      );
    }
  }
}

export const syncService = new SyncService();
