import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { useVoiceDocumentStore } from '../../../features/chat/voice/voiceDocument';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

let pinia: Pinia;
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  mockInvoke('allocate_tts_document', () => '41');
});
afterEach(() => disposePinia(pinia));
const calls = (command: string) => invokeMock.mock.calls.filter(([name]) => name === command);

describe('independent playback document admission', () => {
  it('coalesces concurrent entry points until physical release acknowledges', async () => {
    let release!: () => void;
    mockInvoke('connect_tts_document', () => new Promise<void>(resolve => { release = resolve; }));
    const document = useVoiceDocumentStore();
    let entered = false;
    const first = document.connect().then(id => { entered = true; return id; });
    const second = document.connect();
    await flushPromises();
    expect(entered).toBe(false);
    expect(calls('allocate_tts_document')).toHaveLength(1);
    expect(calls('connect_tts_document')).toHaveLength(1);
    release();
    expect(await Promise.all([first, second])).toEqual(['41', '41']);
    expect(await document.connect()).toBe('41');
    expect(calls('connect_tts_document')).toHaveLength(1);
  });

  it('never activates a late allocation after the document has been disposed', async () => {
    let allocated!: (id: string) => void;
    mockInvoke('allocate_tts_document', () => new Promise<string>(resolve => { allocated = resolve; }));
    const old = useVoiceDocumentStore().connect();
    const rejected = expect(old).rejects.toThrow('voice document disposed');
    disposePinia(pinia);
    pinia = createPinia();
    setActivePinia(pinia);
    mockInvoke('allocate_tts_document', () => '42');
    expect(await useVoiceDocumentStore().connect()).toBe('42');
    allocated('41');
    await rejected;
    expect(calls('connect_tts_document').map(([, args]) => args?.documentId)).toEqual(['42']);
  });
});
