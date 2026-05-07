import { onMounted, onUnmounted, type Ref } from "vue";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { useChatHistoryStore } from "../stores/chatHistoryStore";

export function useMessageEvents(containerRef: Ref<HTMLElement | null>) {
  const historyStore = useChatHistoryStore();

  const handleClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;

    // 1. VCP 按钮点击 (e.g., [[点击按钮:xxx]])
    const vcpButton = target.closest('[data-vcp-button]');
    if (vcpButton) {
      const text = vcpButton.getAttribute('data-vcp-button');
      if (text) historyStore.sendMessage(text);
      return;
    }

    // 2. 外部链接
    const externalLink = target.closest('a[href^="http"]');
    if (externalLink) {
      e.preventDefault();
      const href = externalLink.getAttribute('href');
      if (href) openExternal(href);
      return;
    }

    // 3. 内部锚点（消息引用跳转，暂时留空或按需实现）
    const messageRef = target.closest('a[href^="#msg-"]');
    if (messageRef) {
      e.preventDefault();
      const msgId = messageRef.getAttribute('href')?.replace('#msg-', '');
      if (msgId) {
          // TODO: implement scrollToMessage
      }
      return;
    }
  };

  onMounted(() => {
    if (containerRef.value) {
      containerRef.value.addEventListener("click", handleClick);
    }
  });

  onUnmounted(() => {
    if (containerRef.value) {
      containerRef.value.removeEventListener("click", handleClick);
    }
  });
}
