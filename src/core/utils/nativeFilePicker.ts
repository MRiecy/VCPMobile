import { invoke } from '@tauri-apps/api/core';
import type { PickedFile, PickedFiles, PickFileMode } from 'tauri-plugin-vcp-mobile';
import { isIOS } from './platform';

interface PickerOptions { operationId?: string; maxBytes?: number }
interface PickerAdmission { cancelled: boolean; starting: boolean }

// These entries exist only while the corresponding picker call is in flight.
// Native admission holds one Scene reservation, with no cancellation tombstones.
const admissions = new Map<string, PickerAdmission>();

async function acquire<T>(mode: PickFileMode | undefined, options: PickerOptions, multiple: boolean): Promise<T> {
  const operationId = options.operationId ?? `picker_${crypto.randomUUID()}`;
  const args = { mode, ...options, operationId, ...(multiple ? { multiple: true } : {}) };
  if (!isIOS) return invoke<T>('plugin:vcp-mobile|pick_file', args);
  if (admissions.has(operationId)) throw new Error('busy: file picker operation already exists');

  const admission: PickerAdmission = { cancelled: false, starting: false };
  admissions.set(operationId, admission);
  try {
    const prepared = await invoke<{ reservationToken: string }>('plugin:vcp-mobile|prepare_file_operation', { operationId });
    if (admission.cancelled) throw new Error('cancelled: file picker');
    admission.starting = true;
    return await invoke<T>('plugin:vcp-mobile|pick_file', { ...args, reservationToken: prepared.reservationToken });
  } finally {
    // This also releases a reservation cancelled while its acknowledgement was
    // in flight. An actual picker keeps its files until its own final callback.
    await invoke('plugin:vcp-mobile|cancel_file_operation', { operationId }).catch(console.warn);
    if (admissions.get(operationId) === admission) admissions.delete(operationId);
  }
}

export function pickFile(mode?: PickFileMode, options: PickerOptions = {}): Promise<PickedFile> {
  return acquire<PickedFile>(mode, options, false);
}

export function pickFiles(mode: 'file' | 'gallery', options: PickerOptions = {}): Promise<PickedFiles> {
  return acquire<PickedFiles>(mode, options, true);
}

export async function cancelFileOperation(operationId: string): Promise<void> {
  if (!isIOS) return invoke('plugin:vcp-mobile|cancel_file_operation', { operationId });
  const admission = admissions.get(operationId);
  if (!admission) return;
  admission.cancelled = true;
  // During prepare, its caller will observe this flag before submitting start.
  // Once starting, the already-acknowledged reservation makes cancel-before-start
  // effective even when the native commands arrive in the opposite order.
  if (admission.starting) await invoke('plugin:vcp-mobile|cancel_file_operation', { operationId });
}
