<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Keyboard, Power, RotateCcw } from "lucide-vue-next";
import { useOverlayStore } from "../../../core/stores/overlay";

const props = defineProps<{ keyboardHeight: number }>();

interface TerminalSession {
  operationId: string;
  sessionId: string;
  sessionGeneration: number;
  runtimeGeneration: number;
  pid: number;
  cwd: string;
  shell: string;
  state: "running" | "exited";
  exitCode: number | null;
  cursor: number;
  replayBase64: string;
}

interface TerminalRead {
  operationId: string;
  sessionId: string;
  sessionGeneration: number;
  cursor: number;
  dataBase64: string;
  timedOut: boolean;
  eof: boolean;
  exitCode: number | null;
}

const terminalRoot = ref<HTMLElement | null>(null);
const session = ref<TerminalSession | null>(null);
const starting = ref(false);
const errorMessage = ref("");
const ctrlLatched = ref(false);
const overlayStore = useOverlayStore();
let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let resizeObserver: ResizeObserver | null = null;
let resizeFrame = 0;
let resizeTimer: ReturnType<typeof setTimeout> | null = null;
let cursor = 0;
let pollOwner = 0;
let writeChain = Promise.resolve();
let lastRows = 0;
let lastCols = 0;

const panelStyle = computed(() => ({
  paddingBottom: `${Math.max(0, props.keyboardHeight)}px`,
}));

function operationId(prefix: string): string {
  const suffix =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `${prefix}-${suffix}`;
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

async function writeBytes(bytes: Uint8Array): Promise<void> {
  const identity = session.value;
  if (!identity || identity.state !== "running" || bytes.length === 0) return;
  for (let offset = 0; offset < bytes.length; offset += 16_384) {
    const chunk = bytes.slice(offset, offset + 16_384);
    await invoke("write_vcp_mobile_cli_terminal", {
      request: {
        operationId: operationId("pty-write"),
        sessionId: identity.sessionId,
        sessionGeneration: identity.sessionGeneration,
        dataBase64: encodeBase64(chunk),
      },
    });
  }
}

function queueText(data: string): void {
  if (ctrlLatched.value) {
    ctrlLatched.value = false;
    if (data.length === 1) {
      const code = data.toUpperCase().charCodeAt(0);
      if (code >= 64 && code <= 95) data = String.fromCharCode(code & 0x1f);
    }
  }
  const bytes = new TextEncoder().encode(data);
  writeChain = writeChain.then(() => writeBytes(bytes)).catch((error) => {
    errorMessage.value = String(error);
  });
}

async function openTerminal(): Promise<void> {
  if (!terminal || starting.value) return;
  starting.value = true;
  errorMessage.value = "";
  try {
    fitAddon?.fit();
    const rows = Math.max(2, terminal.rows);
    const cols = Math.max(2, terminal.cols);
    const opened = await invoke<TerminalSession>("open_vcp_mobile_cli_terminal", {
      request: {
        operation_id: operationId("pty-open"),
        cwd: "/workspace",
        rows,
        cols,
      },
    });
    session.value = opened;
    cursor = opened.cursor;
    if (opened.replayBase64) terminal.write(decodeBase64(opened.replayBase64));
    lastRows = rows;
    lastCols = cols;
    const owner = ++pollOwner;
    void pollTerminal(owner);
  } catch (error) {
    errorMessage.value = String(error);
  } finally {
    starting.value = false;
  }
}

async function pollTerminal(owner: number): Promise<void> {
  while (owner === pollOwner) {
    const identity = session.value;
    if (!identity) return;
    try {
      const response = await invoke<TerminalRead>("read_vcp_mobile_cli_terminal", {
        request: {
          operationId: operationId("pty-read"),
          sessionId: identity.sessionId,
          sessionGeneration: identity.sessionGeneration,
          cursor,
          maxBytes: 65_536,
          waitMs: 250,
        },
      });
      if (owner !== pollOwner || session.value?.sessionId !== identity.sessionId) return;
      cursor = response.cursor;
      if (response.dataBase64) terminal?.write(decodeBase64(response.dataBase64));
      if (response.exitCode !== null) {
        session.value = { ...identity, state: "exited", exitCode: response.exitCode };
        return;
      }
      // EOF may arrive just before waitpid publishes the final exit status.
      // Preserve the real exit code without spinning while that handoff settles.
      if (response.eof) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    } catch (error) {
      if (owner === pollOwner) errorMessage.value = String(error);
      return;
    }
  }
}

async function resizeTerminal(): Promise<void> {
  const identity = session.value;
  if (!terminal || !identity || identity.state !== "running") return;
  fitAddon?.fit();
  if (terminal.rows <= 0 || terminal.cols <= 0) return;
  if (terminal.rows === lastRows && terminal.cols === lastCols) return;
  lastRows = terminal.rows;
  lastCols = terminal.cols;
  try {
    await invoke("resize_vcp_mobile_cli_terminal", {
      request: {
        operationId: operationId("pty-resize"),
        sessionId: identity.sessionId,
        sessionGeneration: identity.sessionGeneration,
        rows: terminal.rows,
        cols: terminal.cols,
      },
    });
  } catch (error) {
    errorMessage.value = String(error);
  }
}

function scheduleResize(): void {
  cancelAnimationFrame(resizeFrame);
  if (resizeTimer) clearTimeout(resizeTimer);
  resizeFrame = requestAnimationFrame(() => {
    resizeTimer = setTimeout(() => void resizeTerminal(), 80);
  });
}

async function endSession(confirm = true): Promise<void> {
  const identity = session.value;
  if (!identity) return;
  if (confirm && identity.state === "running") {
    const accepted = await overlayStore.showConfirm({
      title: "结束这个终端会话？",
      message: "将终止此会话的进程树；Agent Jobs 不受影响。",
      isDanger: true,
    });
    if (!accepted) return;
  }
  pollOwner += 1;
  try {
    await invoke("close_vcp_mobile_cli_terminal", {
      request: {
        operationId: operationId("pty-close"),
        sessionId: identity.sessionId,
        sessionGeneration: identity.sessionGeneration,
      },
    });
  } catch (error) {
    errorMessage.value = String(error);
    return;
  }
  session.value = null;
  cursor = 0;
  terminal?.reset();
}

async function newSession(): Promise<void> {
  if (session.value) await endSession(false);
  await openTerminal();
}

function toggleKeyboard(): void {
  const textarea = terminalRoot.value?.querySelector("textarea");
  if (document.activeElement === textarea) (textarea as HTMLTextAreaElement)?.blur();
  else terminal?.focus();
}

watch(() => props.keyboardHeight, scheduleResize);

onMounted(async () => {
  await nextTick();
  if (!terminalRoot.value) return;
  terminal = new Terminal({
    cursorBlink: true,
    convertEol: false,
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: 13,
    scrollback: 5_000,
    allowProposedApi: false,
    theme: { background: "#0b0d0f", foreground: "#d7dde3", cursor: "#5aa9ff" },
  });
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.parser.registerOscHandler(52, () => true);
  terminal.onData(queueText);
  terminal.open(terminalRoot.value);
  resizeObserver = new ResizeObserver(scheduleResize);
  resizeObserver.observe(terminalRoot.value);
  document.fonts?.ready.then(scheduleResize).catch(() => undefined);
  await openTerminal();
});

onBeforeUnmount(() => {
  pollOwner += 1;
  resizeObserver?.disconnect();
  cancelAnimationFrame(resizeFrame);
  if (resizeTimer) clearTimeout(resizeTimer);
  terminal?.dispose();
  terminal = null;
  // Detach only: the PTY remains alive until the explicit "结束会话" action.
});
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col bg-[#0b0d0f]" :style="panelStyle">
    <div class="flex min-h-9 shrink-0 items-center gap-2 border-b border-white/10 px-3 font-mono text-[9px] text-white/55">
      <span>LOCAL · bash · /workspace</span>
      <span class="ml-auto truncate">{{ session?.sessionId.slice(0, 12) || (starting ? "STARTING" : "OFFLINE") }}</span>
      <button
        v-if="session"
        type="button"
        class="min-h-8 px-2 text-red-300/80"
        @click="endSession(true)"
      >
        <Power :size="14" class="inline" /> 结束会话…
      </button>
    </div>

    <div ref="terminalRoot" class="no-swipe min-h-0 flex-1 overflow-hidden p-1" aria-label="本地人工终端" />

    <div v-if="errorMessage" class="shrink-0 border-t border-red-500/30 px-3 py-1 font-mono text-[9px] text-red-300">
      {{ errorMessage }}
    </div>
    <div v-if="session?.state === 'exited'" class="flex shrink-0 items-center border-t border-white/10 px-3 py-1.5 text-[10px] text-white/65">
      Shell 已退出 · code {{ session.exitCode ?? "?" }}
      <button type="button" class="ml-auto min-h-8 px-2 text-blue-300" @click="newSession">
        <RotateCcw :size="14" class="inline" /> 新建会话
      </button>
    </div>

    <div class="grid shrink-0 grid-cols-8 border-t border-white/10 bg-[#111418] text-[11px] text-white/75">
      <button type="button" class="min-h-11" :class="ctrlLatched ? 'bg-blue-500/20 text-blue-300' : ''" @click="ctrlLatched = !ctrlLatched">Ctrl</button>
      <button type="button" class="min-h-11" @click="queueText('\x1b')">Esc</button>
      <button type="button" class="min-h-11" @click="queueText('\t')">Tab</button>
      <button type="button" class="min-h-11 font-mono" @click="queueText('\x1b[D')">←</button>
      <button type="button" class="min-h-11 font-mono" @click="queueText('\x1b[B')">↓</button>
      <button type="button" class="min-h-11 font-mono" @click="queueText('\x1b[A')">↑</button>
      <button type="button" class="min-h-11 font-mono" @click="queueText('\x1b[C')">→</button>
      <button type="button" class="min-h-11" aria-label="显示或隐藏键盘" @click="toggleKeyboard"><Keyboard :size="16" class="mx-auto" /></button>
    </div>
  </section>
</template>

<style scoped>
:deep(.xterm) { height: 100%; }
:deep(.xterm-viewport) { overscroll-behavior: contain; }
</style>
