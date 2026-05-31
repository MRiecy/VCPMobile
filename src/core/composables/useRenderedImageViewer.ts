import { reactive } from "vue";

export interface RenderedImageViewerPayload {
  src: string;
  alt?: string;
  title?: string;
  fileName?: string;
  sourceLabel?: string;
}

const state = reactive({
  isOpen: false,
  src: "",
  alt: "",
  title: "",
  fileName: "",
  sourceLabel: "",
});

export function sanitizeRenderedImageSrc(value: string): string {
  const src = value.trim();
  if (!src) return "";
  if (/^https?:\/\//i.test(src)) return src;
  if (/^data:image\//i.test(src)) return src;
  if (/^(blob:|file:|content:|asset:)/i.test(src)) return src;
  if (/^[./]/.test(src)) {
    try {
      return new URL(src, window.location.href).href;
    } catch {
      return "";
    }
  }
  return "";
}

export function openRenderedImageViewer(
  payload: RenderedImageViewerPayload,
): void {
  const src = sanitizeRenderedImageSrc(payload.src || "");
  if (!src) return;

  state.src = src;
  state.alt = payload.alt || "";
  state.title = payload.title || "";
  state.fileName = payload.fileName || "";
  state.sourceLabel = payload.sourceLabel || "AI 渲染图片";
  state.isOpen = true;
}

export function closeRenderedImageViewer(): void {
  state.isOpen = false;
}

export function useRenderedImageViewer() {
  return {
    state,
    openRenderedImageViewer,
    closeRenderedImageViewer,
  };
}
