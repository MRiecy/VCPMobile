import { ref } from 'vue';
import { Copy, Check } from 'lucide-vue-next';
import type { VcpNotification } from '../../../core/stores/notification';

export const useNotificationClipboard = () => {
  const copiedId = ref<string | null>(null);

  const buildCopyText = (item: VcpNotification) => {
    return item.rawPayload
      ? JSON.stringify(item.rawPayload, null, 2)
      : `${item.title}\n${item.message}`;
  };

  const copyContent = async (item: VcpNotification) => {
    try {
      await navigator.clipboard.writeText(buildCopyText(item));
      copiedId.value = item.id;

      window.setTimeout(() => {
        if (copiedId.value === item.id) {
          copiedId.value = null;
        }
      }, 2000);
    } catch (error) {
      console.error('[useNotificationClipboard] Copy failed:', error);
    }
  };

  const getCopyIcon = (itemId: string) => copiedId.value === itemId ? Check : Copy;

  return {
    copiedId,
    buildCopyText,
    copyContent,
    getCopyIcon
  };
};
