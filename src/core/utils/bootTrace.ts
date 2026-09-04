/**
 * 启动分段计时（BootTrace）前端侧。
 *
 * 与 Rust 侧 `boot_trace.rs` 配对：前端 marks 以 navigationStart 为 t0，
 * Rust marks 以 setup 起点为 t0，两者时钟不同源，上报时按各自时间轴分段展示。
 * READY 后由 appLifecycle 触发一次合并上报：
 *   - dev 下 console.table 打印分段表；
 *   - 始终通过 `save_boot_trace` 落盘（app_data/boot_trace.jsonl）+ logcat，
 *     供 `pnpm android:debug:logs` / run-as 拉取做冷启动 A/B。
 */

import { invoke } from '@tauri-apps/api/core';

export interface BootMark {
  name: string;
  atMs: number;
}

interface RustBootMark {
  name: string;
  atMs: number;
}

const marks: BootMark[] = [];
let reportScheduled = false;

export function bootMark(name: string): void {
  marks.push({ name, atMs: Math.round(performance.now() * 100) / 100 });
}

/** 包装 Promise，在完成（而非拒绝）时打一个 mark；拒绝原样透传。 */
export function trackBootStage<T>(name: string, p: Promise<T>): Promise<T> {
  return p.then((value) => {
    bootMark(name);
    return value;
  });
}

/** 合并前端 + Rust 轨迹并上报。幂等：重复调用只执行一次。 */
export async function reportBootTrace(): Promise<void> {
  if (reportScheduled) return;
  reportScheduled = true;

  try {
    const rustMarks = await invoke<RustBootMark[]>('get_boot_trace');
    const report = {
      frontend: marks,
      rust: rustMarks,
      reportedAt: new Date().toISOString(),
    };

    // adb / 测试脚本可通过 window 全局直接取值
    (window as unknown as { __VCP_BOOT_TRACE__?: unknown }).__VCP_BOOT_TRACE__ = report;

    if (import.meta.env.DEV) {
      console.groupCollapsed('[BootTrace] 启动分段耗时');
      console.table(marks);
      console.table(rustMarks);
      console.groupEnd();
    }

    await invoke('save_boot_trace', { payload: JSON.stringify(report) });
  } catch (error) {
    // 埋点失败永不影响主流程
    console.warn('[BootTrace] report failed:', error);
  }
}
