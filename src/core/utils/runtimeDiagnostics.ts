import type { App } from "vue";
import { invoke } from "@tauri-apps/api/core";

type DiagnosticLevel = "error" | "warn";

interface DiagnosticPayload {
  level: DiagnosticLevel;
  source: string;
  message: string;
  stack?: string;
  location?: string;
  windowLabel?: string;
  timestamp: number;
}

const RECENT_EVENT_TTL_MS = 5_000;
const MAX_PENDING_REPORTS = 8;
const MAX_TEXT_CHARS = 8_000;
const recentEvents = new Map<string, number>();
let pendingReports = 0;

function sanitizeText(value: string): string {
  const redacted = value
    .replace(/(authorization\s*[:=]\s*bearer\s+)[^\s,;]+/gi, "$1[REDACTED]")
    .replace(/((?:api[_-]?key|access[_-]?token|password|secret)\s*[:=]\s*)[^\s,;]+/gi, "$1[REDACTED]");
  return redacted.length > MAX_TEXT_CHARS
    ? `${redacted.slice(0, MAX_TEXT_CHARS)}\n…[已截断]`
    : redacted;
}

function stringifyUnknown(value: unknown): string {
  if (value instanceof Error) return sanitizeText(value.message || value.name);
  if (typeof value === "string") return sanitizeText(value);
  if (value === null || value === undefined) return String(value);
  if (typeof value !== "object") return sanitizeText(String(value));

  try {
    if (Array.isArray(value)) {
      return sanitizeText(
        `[Array(${value.length})] ${value
          .slice(0, 8)
          .map(describeConsoleArgument)
          .join(", ")}`,
      );
    }
    const record = value as Record<string, unknown>;
    const entries = Object.keys(record)
      .slice(0, 12)
      .map((key) => `${key}=${describeConsoleArgument(record[key])}`);
    return sanitizeText(`{${entries.join(", ")}}`);
  } catch {
    return Object.prototype.toString.call(value);
  }
}

function stackFromUnknown(value: unknown): string | undefined {
  return value instanceof Error && value.stack
    ? sanitizeText(value.stack)
    : undefined;
}

function describeConsoleArgument(value: unknown): string {
  if (value instanceof Error) return sanitizeText(`${value.name}: ${value.message}`);
  if (typeof value === "string") return sanitizeText(value);
  if (value === null || value === undefined) return String(value);
  return Object.prototype.toString.call(value);
}

function currentLocation(): string | undefined {
  if (typeof window === "undefined") return undefined;
  return `${window.location.pathname}${window.location.hash}`;
}

function submitDiagnostic(payload: DiagnosticPayload): void {
  const signature = `${payload.source}:${payload.message}:${payload.stack || ""}`;
  const now = Date.now();
  const lastSeen = recentEvents.get(signature) || 0;
  if (
    now - lastSeen < RECENT_EVENT_TTL_MS ||
    pendingReports >= MAX_PENDING_REPORTS
  ) {
    return;
  }

  recentEvents.set(signature, now);
  pendingReports += 1;
  void invoke("record_frontend_diagnostic", { event: payload })
    .catch(() => undefined)
    .finally(() => {
      pendingReports = Math.max(0, pendingReports - 1);
      for (const [key, timestamp] of recentEvents) {
        if (now - timestamp > RECENT_EVENT_TTL_MS * 4) recentEvents.delete(key);
      }
    });
}

/**
 * 在 Vue 挂载前注册异常捕获，将 release WebView 中原本不可见的错误持久化到 Rust 日志。
 */
export function installRuntimeDiagnostics(
  app: App<Element>,
  windowLabel?: string,
): void {
  const previousVueHandler = app.config.errorHandler;
  app.config.errorHandler = (error, instance, info) => {
    submitDiagnostic({
      level: "error",
      source: "vue",
      message: `${info}: ${stringifyUnknown(error)}`,
      stack: stackFromUnknown(error),
      location: currentLocation(),
      windowLabel,
      timestamp: Date.now(),
    });
    previousVueHandler?.(error, instance, info);
  };

  if (typeof window === "undefined") return;

  const originalConsoleError = console.error.bind(console);
  console.error = (...args: unknown[]) => {
    originalConsoleError(...args);
    const firstError = args.find((value) => value instanceof Error);
    submitDiagnostic({
      level: "error",
      source: "console",
      message: args.map(describeConsoleArgument).join(" ") || "空 console.error",
      stack: stackFromUnknown(firstError),
      location: currentLocation(),
      windowLabel,
      timestamp: Date.now(),
    });
  };

  window.addEventListener(
    "error",
    (event) => {
      const target = event.target as HTMLElement | null;
      const resourceUrl =
        target && "src" in target
          ? String((target as HTMLImageElement | HTMLScriptElement).src || "")
          : "";
      submitDiagnostic({
        level: "error",
        source: resourceUrl ? "resource" : "window",
        message: resourceUrl
          ? `资源加载失败: ${resourceUrl}`
          : event.message || "未命名的 window error",
        stack: event.error instanceof Error ? event.error.stack : undefined,
        location: resourceUrl || currentLocation(),
        windowLabel,
        timestamp: Date.now(),
      });
    },
    true,
  );

  window.addEventListener("unhandledrejection", (event) => {
    submitDiagnostic({
      level: "error",
      source: "unhandledrejection",
      message: stringifyUnknown(event.reason),
      stack: stackFromUnknown(event.reason),
      location: currentLocation(),
      windowLabel,
      timestamp: Date.now(),
    });
  });
}
