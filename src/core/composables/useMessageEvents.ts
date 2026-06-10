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

    // 1.5 拦截 AI 回复中生成的内嵌 <button> 元素
    const aiButton = target.closest('button') as HTMLButtonElement | null;
    if (aiButton) {
      e.preventDefault();
      e.stopPropagation();

      // 如果按钮已被禁用，直接拦截，防止重复点击
      if (aiButton.disabled) {
        return;
      }

      // 提取发送文本（优先级：data-send 属性 > 按钮 textContent）
      const sendText = aiButton.getAttribute('data-send') || aiButton.textContent?.trim();
      if (sendText) {
        let finalSendText = `[[点击按钮:${sendText}]]`;

        // 超长文本截断（防超限）
        if (finalSendText.length > 500) {
          const maxTextLength = 500 - '[[点击按钮:]]'.length;
          const truncatedText = sendText.substring(0, maxTextLength);
          finalSendText = `[[点击按钮:${truncatedText}]]`;
        }

        // 按钮物理禁用与状态置灰反馈（与桌面端一致）
        aiButton.disabled = true;
        aiButton.style.opacity = '0.6';
        aiButton.style.cursor = 'not-allowed';
        const originalText = aiButton.textContent || '';
        aiButton.textContent = originalText + ' ✓';

        // 发送消息
        historyStore.sendMessage(finalSendText);
      }
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

    // 4. 气泡内普通图片点击劫持 (排除带有 vcp-emoticon 的表情包)
    if (target.tagName.toLowerCase() === "img") {
      const isEmoticon = target.classList.contains("vcp-emoticon");
      if (!isEmoticon) {
        e.preventDefault();
        e.stopPropagation();

        const src = target.getAttribute("src") || "";
        const alt = target.getAttribute("alt") || "";
        const title = target.getAttribute("title") || "";

        // 动态引入查看器 Composable，消灭潜在的 Vue 组件循环引用
        import("./useRenderedImageViewer")
          .then(({ openRenderedImageViewer }) => {
            openRenderedImageViewer({
              src,
              alt,
              title,
              sourceLabel: "聊天图片",
            });
          })
          .catch((err) => {
            console.error("[useMessageEvents] Failed to open RenderedImageViewer:", err);
          });
        return;
      }
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
