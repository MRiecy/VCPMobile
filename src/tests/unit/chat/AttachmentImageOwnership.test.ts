import { flushPromises, mount } from '@vue/test-utils';
import { describe, expect, it, vi } from 'vitest';
import AttachmentViewer from '@/features/chat/attachment/AttachmentViewer.vue';
import ImageAttachment from '@/features/chat/attachment/types/ImageAttachment.vue';
import type { Attachment } from '@/core/types/chat';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));
vi.mock('@/core/composables/useModalHistory', () => ({
  useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }),
}));

const file: Attachment = {
  name: 'photo.avif', type: 'image/avif', size: 100, hash: 'hash',
  src: '/cas/photo.avif', internalPath: '/cas/photo.avif', status: 'done',
};
const mountImage = (kind: 'viewer' | 'thumbnail') => kind === 'viewer'
  ? mount(AttachmentViewer, { props: { file, isOpen: true }, global: { stubs: { Transition: true } } })
  : mount(ImageAttachment, {
    props: { file, index: 0 },
    global: { stubs: { AttachmentPreviewBase: { template: '<div><slot /></div>' } } },
  });

describe.each(['viewer', 'thumbnail'] as const)('%s image ownership', kind => {
  it.each([false, true])('releases only owned images on close (owned=%s)', async owned => {
    const path = owned ? '/image-work/derived.png' : file.src;
    mockInvoke('get_attachment_image', () => ({ path, owned }));
    const wrapper = mountImage(kind);
    await flushPromises();
    expect(wrapper.get('img').attributes('src')).toBe(`asset://${path}`);
    wrapper.unmount();
    await flushPromises();
    const releases = invokeMock.mock.calls.filter(([command]) => command === 'release_attachment_image');
    expect(releases).toEqual(owned ? [['release_attachment_image', { path }]] : []);
  });

  it.each([false, true])('handles late results without releasing a CAS original (owned=%s)', async owned => {
    let finish!: (result: { path: string; owned: boolean }) => void;
    mockInvoke('get_attachment_image', () => new Promise(resolve => { finish = resolve; }));
    const wrapper = mountImage(kind);
    await flushPromises();
    wrapper.unmount();
    const path = owned ? '/image-work/late.png' : file.src;
    finish({ path, owned });
    await flushPromises();
    const releases = invokeMock.mock.calls.filter(([command]) => command === 'release_attachment_image');
    expect(releases).toEqual(owned ? [['release_attachment_image', { path }]] : []);
  });
});
