import { afterEach, vi } from 'vitest';

export type MockInvokeHandler = (args?: Record<string, unknown>) => unknown | Promise<unknown>;

const invokeHandlers = new Map<string, MockInvokeHandler>();

export const invokeMock = vi.fn((command: string, args?: Record<string, unknown>) => {
  const handler = invokeHandlers.get(command);
  if (handler) {
    return Promise.resolve(handler(args));
  }
  return Promise.resolve(undefined);
});

export function mockInvoke(command: string, handler: MockInvokeHandler): void {
  invokeHandlers.set(command, handler);
}

export function clearInvokeMocks(): void {
  invokeHandlers.clear();
  invokeMock.mockClear();
}

export const convertFileSrcMock = vi.fn((path: string) => `asset://${path}`);

export const channelInstances: MockChannel<unknown>[] = [];

export class MockChannel<T = unknown> {
  public onmessage?: (message: T) => void;

  constructor() {
    channelInstances.push(this as MockChannel<unknown>);
  }

  emit(message: T): void {
    this.onmessage?.(message);
  }
}

const listeners = new Map<string, Array<(event: { payload: unknown }) => void>>();

export const listenMock = vi.fn(async (event: string, handler: (event: { payload: unknown }) => void) => {
  const eventListeners = listeners.get(event) ?? [];
  eventListeners.push(handler);
  listeners.set(event, eventListeners);

  return () => {
    const current = listeners.get(event) ?? [];
    listeners.set(event, current.filter((item) => item !== handler));
  };
});

export function emitTauriEvent(event: string, payload: unknown): void {
  for (const handler of listeners.get(event) ?? []) {
    handler({ payload });
  }
}

export function clearTauriMocks(): void {
  clearInvokeMocks();
  convertFileSrcMock.mockClear();
  listenMock.mockClear();
  channelInstances.length = 0;
  listeners.clear();
}

afterEach(() => {
  clearTauriMocks();
});

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
  convertFileSrc: convertFileSrcMock,
  Channel: MockChannel,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock,
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: vi.fn(() => ({
    close: vi.fn(),
    hide: vi.fn(),
    show: vi.fn(),
    setFocus: vi.fn(),
  })),
}));

vi.mock('@tauri-apps/api/webviewWindow', () => ({
  getCurrentWebviewWindow: vi.fn(() => ({
    listen: vi.fn(),
    close: vi.fn(),
    hide: vi.fn(),
    show: vi.fn(),
    setFocus: vi.fn(),
  })),
  WebviewWindow: vi.fn().mockImplementation(() => ({
    once: vi.fn(),
    show: vi.fn(),
    setFocus: vi.fn(),
    close: vi.fn(),
  })),
}));

vi.mock('tauri-plugin-vcp-mobile', () => ({
  setKeepScreenOn: vi.fn(() => Promise.resolve()),
  clearKeepScreenOn: vi.fn(() => Promise.resolve()),
  startStreamService: vi.fn(() => Promise.resolve()),
  stopStreamService: vi.fn(() => Promise.resolve()),
  pickFile: vi.fn(() => Promise.resolve(undefined)),
  openFileNative: vi.fn(() => Promise.resolve()),
  shareFileNative: vi.fn(() => Promise.resolve()),
  saveImageToGallery: vi.fn(() => Promise.resolve({ uri: '', displayName: '', mimeType: '', size: 0 })),
  saveImageFromPath: vi.fn(() => Promise.resolve({ uri: '', displayName: '', mimeType: '', size: 0 })),
  writeTempFile: vi.fn(() => Promise.resolve('')),
  checkRootAccess: vi.fn(() => Promise.resolve({ isRoot: false })),
  runRootCommand: vi.fn(() => Promise.resolve({ success: false, output: '' })),
  launchRootManager: vi.fn(() => Promise.resolve({ success: false })),
}));
