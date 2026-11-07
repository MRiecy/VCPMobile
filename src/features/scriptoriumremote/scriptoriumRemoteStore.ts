import { computed, ref, watch } from 'vue';
import { defineStore } from 'pinia';
import {
  bridgeBaseUrl,
  bridgeConfigSource,
  bridgeToken,
  refreshBridgeConfig,
  saveBridgeConfig,
} from '../../core/bridge/bridgeConfig';

export interface RemoteDocMeta {
  name: string;
  kind: 'docx' | 'pptx';
  size: number;
  mtime: number;
}

export interface RemoteDoc {
  name: string;
  kind: 'docx' | 'pptx';
  manifest: Record<string, any> | null;
  sources: Record<string, string>;
  checkpoints: any[] | null;
}

/** 桌面文坊（VDOCX/VPPTX）的浏览：文档库、正文、文脉、修订。 */
export const useScriptoriumRemoteStore = defineStore('scriptoriumRemote', () => {
  refreshBridgeConfig();
  // 桥接地址/Token 统一由 core/bridge/bridgeConfig 管理（默认跟随同步设置中的桌面地址）
  const baseUrl = computed(() => bridgeBaseUrl.value);
  const token = computed(() => bridgeToken.value);
  const configSource = computed(() => bridgeConfigSource.value);
  const docs = ref<RemoteDocMeta[]>([]);
  const doc = ref<RemoteDoc | null>(null);
  const loading = ref(false);
  const error = ref('');
  const history = ref<Array<{ id: string; at: number; summary: string; length: number; isCurrent?: boolean }>>([]);

  // 桥接端点变化（例如在「远程桌面 / V工程」里改了地址）时清空旧数据，避免展示旧桌面的内容
  watch([bridgeBaseUrl, bridgeToken], () => {
    docs.value = [];
    doc.value = null;
    history.value = [];
    error.value = '';
  });

  /** 文坊页内设置：保存桥接地址/Token（写入统一配置，三个功能页面共享）。 */
  function saveSettings(next: { baseUrl: string; token: string }): void {
    saveBridgeConfig(next);
  }

  const title = computed(() => {
    const manifest = doc.value?.manifest || {};
    const meta = (manifest.manifest || manifest) as Record<string, any>;
    return String(meta.title || doc.value?.name || '');
  });

  const markdown = computed(() => doc.value?.sources?.['source/document.md'] || '');
  const css = computed(() => doc.value?.sources?.['source/document.css'] || '');

  const lineage = computed(() => {
    const list = doc.value?.checkpoints;
    if (!Array.isArray(list)) return [];
    return [...list].sort((a, b) => Number(b.createdAt || 0) - Number(a.createdAt || 0));
  });

  async function call<T>(path: string, init?: { method?: string; body?: unknown }): Promise<T> {
    const response = await fetch(`${baseUrl.value}${path}`, {
      method: init?.method || 'GET',
      headers: {
        Authorization: `Bearer ${token.value}`,
        ...(init?.body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: init?.body === undefined ? undefined : JSON.stringify(init.body),
    });
    const text = await response.text();
    let payload: any = null;
    try {
      payload = text ? JSON.parse(text) : null;
    } catch {
      throw new Error(`Bridge returned non-JSON (${response.status})`);
    }
    if (!response.ok || payload?.ok === false) {
      throw new Error(payload?.error?.message || `Bridge error ${response.status}`);
    }
    return payload as T;
  }

  async function loadDocs(): Promise<void> {
    loading.value = true;
    error.value = '';
    try {
      const data = await call<{ docs: RemoteDocMeta[] }>('/scriptorium/docs');
      docs.value = data.docs || [];
    } catch (e) {
      error.value = String((e as Error)?.message || e);
    } finally {
      loading.value = false;
    }
  }

  async function loadHistory(name: string): Promise<void> {
    try {
      const data = await call<{
        history: Array<{ id: string; at: number; summary: string; length: number; isCurrent?: boolean }>;
      }>(`/scriptorium/history?name=${encodeURIComponent(name)}`);
      history.value = data.history || [];
    } catch {
      history.value = [];
    }
  }

  async function rollback(name: string, historyId: string): Promise<void> {
    await call('/scriptorium/rollback', { method: 'POST', body: { name, historyId } });
    await new Promise((resolve) => setTimeout(resolve, 1200));
    await openDoc(name);
  }

  async function openDoc(name: string): Promise<void> {
    loading.value = true;
    error.value = '';
    try {
      const data = await call<{ doc: RemoteDoc }>(`/scriptorium/doc?name=${encodeURIComponent(name)}`);
      doc.value = data.doc;
      await loadHistory(name);
    } catch (e) {
      error.value = String((e as Error)?.message || e);
      doc.value = null;
    } finally {
      loading.value = false;
    }
  }

  async function exportDoc(
    name: string,
    format: 'pdf' | 'html-flow' | 'html-paged' | 'project',
  ): Promise<{ fileName: string; bytes: number; mime: string; base64: string }> {
    if (format === 'project') {
      const data = await call<{ fileName: string; bytes: number; mime: string; base64: string }>(
        `/scriptorium/download?name=${encodeURIComponent(name)}`,
      );
      return data;
    }
    const data = await call<{ fileName: string; bytes: number; mime: string; base64: string }>(
      '/scriptorium/export',
      { method: 'POST', body: { name, format } },
    );
    if (format === 'html-flow' || format === 'html-paged') {
      const suffix = format === 'html-flow' ? '-flow' : '-paged';
      data.fileName = data.fileName.replace(/\.html$/i, `${suffix}.html`);
    }
    return data;
  }

  async function applyEdit(name: string, content: string): Promise<{ mode: string; revision?: number }> {
    const previous = markdown.value || '';
    const isAppend = previous.length > 0 && content.startsWith(previous);
    const mode = isAppend ? 'append' : 'replaceAll';
    const payload = isAppend ? content.slice(previous.length) : content;
    const data = await call<{ result: any }>('/scriptorium/apply', {
      method: 'POST',
      body: { name, mode, content: payload, summary: '手机端编辑', maid: 'VCPMobile' },
    });
    const text = Array.isArray(data.result?.content)
      ? data.result.content.map((item: any) => item.text || '').join('\n')
      : '';
    const match = text.match(/修订号\*\*：(\d+)/) || text.match(/修订号：(\d+)/);
    return { mode, revision: match ? Number(match[1]) : undefined };
  }

  function closeDoc(): void {
    doc.value = null;
  }

  return {
    baseUrl,
    token,
    configSource,
    saveSettings,
    docs,
    doc,
    loading,
    error,
    history,
    title,
    markdown,
    css,
    lineage,
    loadDocs,
    openDoc,
    exportDoc,
    loadHistory,
    rollback,
    applyEdit,
    closeDoc,
  };
});
