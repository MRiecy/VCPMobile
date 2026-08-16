/**
 * presentationMenu — 「消息呈现」长按菜单的共享入口
 *
 * 从 ChatView.vue 原样提取（行为零变化）：
 * - ChatView 的亮暗按钮 v-longpress.suppress-click 继续调用；
 * - 教学引导 theme-longpress 的「长按真实弹出菜单」步骤调用同一入口。
 */
import { useOverlayStore } from '../../core/stores/overlay';
import { useNotificationStore } from '../../core/stores/notification';
import {
  CHAT_PRESENTATION_OPTIONS,
  useThemeStore,
  type ChatPresentationMode,
} from '../../core/stores/theme';

export function handlePresentationSelection(mode: ChatPresentationMode): void {
  const themeStore = useThemeStore();
  const result = themeStore.setPresentationMode(mode);
  if (!result.ok) {
    useNotificationStore().addNotification({
      id: 'chat-presentation-save-failed',
      type: 'error',
      title: '消息呈现切换失败',
      message: result.error || '请检查本地存储后重试',
      toastOnly: true,
    });
  }
}

export function openPresentationMenu(): void {
  try {
    navigator.vibrate?.(25);
  } catch {
    // Haptics are an optional enhancement; unsupported devices stay silent.
  }

  const themeStore = useThemeStore();
  useOverlayStore().openContextMenu(
    CHAT_PRESENTATION_OPTIONS.map((option) => ({
      label: option.label,
      selected: themeStore.presentationMode === option.value,
      handler: () => handlePresentationSelection(option.value),
    })),
    '消息呈现',
  );
}
