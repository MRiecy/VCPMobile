import { useSettingsStore } from "../stores/settings";

/**
 * SyncService
 * 
 * 职责：负责移动端与桌面端的数据同步逻辑。
 * 目前主要支持：话题上行同步 (pushTopicToDesktop)。
 */
export class SyncService {
  private normalizeBaseUrl(rawUrl?: string): string {
    const source = rawUrl?.trim() || "http://127.0.0.1:5974";
    const normalized = source.replace(/\/+$/, "");
    return normalized.endsWith("/api/mobile-sync")
      ? normalized
      : `${normalized}/api/mobile-sync`;
  }

  private getBaseUrl(customUrl?: string): string {
    const settingsStore = useSettingsStore();
    return this.normalizeBaseUrl(
      customUrl || settingsStore.settings?.syncServerUrl,
    );
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
