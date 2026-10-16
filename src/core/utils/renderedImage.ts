import type { RenderedImageViewerPayload } from "../composables/useRenderedImageViewer";
import { sanitizeRenderedImageSrc } from "../composables/useRenderedImageViewer";
import { exportCanvas, exportSvg } from './renderedImageExport';

const URL_IN_CSS_RE = /url\((["']?)(.*?)\1\)/i;

function absoluteUrl(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  try {
    return sanitizeRenderedImageSrc(new URL(trimmed, window.location.href).href);
  } catch {
    return sanitizeRenderedImageSrc(trimmed);
  }
}

function fileNameFromUrl(url: string): string {
  if (!url || url.startsWith("data:") || url.startsWith("blob:")) return "";
  try {
    const parsed = new URL(url, window.location.href);
    const segment = parsed.pathname.split("/").filter(Boolean).pop() || "";
    return decodeURIComponent(segment);
  } catch {
    const segment = url.split(/[/?#]/).filter(Boolean).pop() || "";
    try {
      return decodeURIComponent(segment);
    } catch {
      return segment;
    }
  }
}

async function imagePayloadFromElement(
  element: Element,
  signal?: AbortSignal,
): Promise<RenderedImageViewerPayload | null> {
  if (element instanceof HTMLImageElement) {
    const src = absoluteUrl(
      element.dataset.vcpImageSrc ||
        element.currentSrc ||
        element.src ||
        element.getAttribute("src") ||
        "",
    );
    if (!src) return null;
    return {
      src,
      alt: element.alt || "",
      title: element.title || "",
      fileName: fileNameFromUrl(src),
    };
  }

  if (element instanceof SVGImageElement) {
    const href =
      element.href?.baseVal ||
      element.getAttribute("href") ||
      element.getAttribute("xlink:href") ||
      "";
    const src = absoluteUrl(href);
    if (!src) return null;
    return {
      src,
      title: element.getAttribute("title") || "",
      fileName: fileNameFromUrl(src),
    };
  }

  if (element instanceof HTMLCanvasElement) {
    const src = URL.createObjectURL(await exportCanvas(element, signal));
    return {
      src,
      ownedObjectUrl: true,
      title: element.getAttribute("aria-label") || "",
      fileName: "vcp-canvas.png",
    };
  }

  if (element instanceof SVGSVGElement) {
    const src = URL.createObjectURL(await exportSvg(element, signal));
    return {
      src,
      ownedObjectUrl: true,
      title:
        element.getAttribute("aria-label") ||
        element.querySelector("title")?.textContent ||
        "",
      fileName: "vcp-svg.png",
    };
  }

  return null;
}

function backgroundPayloadFromElement(
  element: Element,
): RenderedImageViewerPayload | null {
  if (!(element instanceof HTMLElement || element instanceof SVGElement))
    return null;
  const style = window.getComputedStyle(element);
  const match = style.backgroundImage.match(URL_IN_CSS_RE);
  const rawUrl = match?.[2];
  if (!rawUrl) return null;
  const src = absoluteUrl(rawUrl);
  if (!src) return null;

  return {
    src,
    title:
      element.getAttribute("aria-label") || element.getAttribute("title") || "",
    fileName: fileNameFromUrl(src),
  };
}

export async function findRenderedImagePayload(
  target: EventTarget | null,
  container?: HTMLElement | null,
  signal?: AbortSignal,
): Promise<RenderedImageViewerPayload | null> {
  if (!(target instanceof Element)) return null;

  const directImage = target.closest("img, image, canvas, svg");
  if (directImage && (!container || container.contains(directImage))) {
    const payload = await imagePayloadFromElement(directImage, signal);
    if (payload) return payload;
  }

  let current: Element | null = target;
  while (current && (!container || container.contains(current))) {
    const payload = backgroundPayloadFromElement(current);
    if (payload) return payload;
    if (current === container) break;
    current = current.parentElement;
  }

  return null;
}
