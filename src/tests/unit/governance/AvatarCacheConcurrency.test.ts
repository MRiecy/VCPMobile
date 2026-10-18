import { describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAvatarStore } from '../../../core/stores/avatar';
import { mockInvoke } from '../../mocks/tauri';

describe('avatar cache concurrency', () => {
  it('does not let an older avatar read overwrite a refresh that finished first', async () => {
    setActivePinia(createPinia());
    let resolveOldRead!: (value: unknown) => void;
    mockInvoke('get_avatar', () => new Promise((resolve) => {
      resolveOldRead = resolve;
    }));

    const createObjectUrl = vi.mocked(URL.createObjectURL);
    let objectUrlSequence = 0;
    createObjectUrl.mockImplementation(() => `blob:avatar-${++objectUrlSequence}`);

    try {
      const store = useAvatarStore();
      const oldRead = store.getAvatarUrl('agent', 'agent-1');
      await vi.waitFor(() => expect(resolveOldRead).toBeTypeOf('function'));

      mockInvoke('get_avatar', () => ({
        avatar_hash: 'new-hash',
        mime_type: 'image/png',
        image_data: [2],
        dominant_color: '#123456',
        updated_at: 2,
      }));
      const refreshed = await store.refreshAvatar('agent', 'agent-1', 'new-hash');

      resolveOldRead({
        avatar_hash: 'old-hash',
        mime_type: 'image/png',
        image_data: [1],
        dominant_color: '#654321',
        updated_at: 1,
      });

      expect(refreshed).toBe('blob:avatar-1');
      await expect(oldRead).resolves.toBe('blob:avatar-1');
      expect(store.cache.get('agent:agent-1')?.blobUrl).toBe('blob:avatar-1');
    } finally {
      createObjectUrl.mockImplementation(() => 'blob:mock');
    }
  });

  it('invalidates an unknown in-flight avatar read when metadata becomes authoritative', async () => {
    setActivePinia(createPinia());
    let resolveOldRead!: (value: unknown) => void;
    mockInvoke('get_avatar', () => new Promise((resolve) => {
      resolveOldRead = resolve;
    }));

    const createObjectUrl = vi.mocked(URL.createObjectURL);
    createObjectUrl.mockImplementation(() => 'blob:metadata-current');

    try {
      const store = useAvatarStore();
      const oldRead = store.getAvatarUrl('agent', 'agent-2');

      mockInvoke('batch_get_avatars', () => [{
        ownerType: 'agent',
        ownerId: 'agent-2',
        avatarHash: 'current-hash',
        dominantColor: '#123456',
        updatedAt: 2,
      }]);
      await store.refreshMetadata();

      resolveOldRead({
        avatar_hash: 'old-hash',
        mime_type: 'image/png',
        image_data: [1],
        dominant_color: '#654321',
        updated_at: 1,
      });
      await expect(oldRead).resolves.toBe('');
      expect(store.metadata.get('agent:agent-2')?.avatarHash).toBe('current-hash');

      mockInvoke('get_avatar', () => ({
        avatar_hash: 'current-hash',
        mime_type: 'image/png',
        image_data: [2],
        dominant_color: '#123456',
        updated_at: 2,
      }));
      await expect(store.getAvatarUrl('agent', 'agent-2')).resolves.toBe('blob:metadata-current');
      expect(store.cache.get('agent:agent-2')?.avatarHash).toBe('current-hash');
    } finally {
      createObjectUrl.mockImplementation(() => 'blob:mock');
    }
  });
});
