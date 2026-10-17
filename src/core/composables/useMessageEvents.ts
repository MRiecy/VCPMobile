import { onMounted, onUnmounted, type Ref } from "vue";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { useChatHistoryStore } from "../stores/chatHistoryStore";
import { openRenderedImageViewer } from "./useRenderedImageViewer";
import { scrollToMessageById } from "../utils/scrollToMessage";
import { findRenderedImagePayload } from '../utils/renderedImage';
import { useNotificationStore } from '../stores/notification';
import { MERMAID_CONTENT_SELECTOR } from '../utils/astRenderer';

export function useMessageEvents(
  containerRef: Ref<HTMLElement | null>,
  openMermaid: (svgHtml: string, sourceCode: string) => void,
) {
  const historyStore = useChatHistoryStore();
  const notification = useNotificationStore();
  let imageController: AbortController | null = null;
  let eventContainer: HTMLElement | null = null;

  const handleClick = async (e: MouseEvent) => {
    const target = e.target;
    const container = containerRef.value;
    if (!(target instanceof Element) || !container?.contains(target)) return;

    // 图表身份先于按钮、链接和图片；交互不依赖异步渲染是否安装了 wrapper。
    const diagram = target.closest<HTMLElement>(MERMAID_CONTENT_SELECTOR);
    if (diagram && container.contains(diagram)) {
      e.preventDefault();
      e.stopPropagation();
      const svg = diagram.querySelector('svg');
      if (svg && diagram.closest('[data-message-render-region]')?.getAttribute('data-message-render-region') === 'stable') {
        openMermaid(svg.outerHTML, diagram.dataset.mermaidSource ?? '');
      }
      return;
    }

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

    // 3. 内部锚点（消息引用跳转）：仅当消息在当前已加载窗口内时可定位
    const messageRef = target.closest('a[href^="#msg-"]');
    if (messageRef) {
      e.preventDefault();
      const msgId = messageRef.getAttribute('href')?.replace('#msg-', '');
      if (msgId) {
        scrollToMessageById(msgId);
      }
      return;
    }

    // 4. 气泡内普通图片点击劫持 (排除带有 vcp-emoticon 的表情包以及消息附件缩略图)
    const imageElement = target.closest('img, image, svg, canvas');
    if (imageElement) {
      const isEmoticon = imageElement.classList.contains("vcp-emoticon");
      const isAttachment = target.closest(".vcp-attachment-preview") !== null;

      if (!isEmoticon && !isAttachment) {
        e.preventDefault();
        e.stopPropagation();

        imageController?.abort();
        const controller = new AbortController();
        imageController = controller;
        try {
          const payload = await findRenderedImagePayload(target, containerRef.value, controller.signal);
          if (payload) {
            if (controller.signal.aborted || !containerRef.value?.contains(imageElement)) {
              if (payload.ownedObjectUrl) URL.revokeObjectURL(payload.src);
            } else openRenderedImageViewer({ ...payload, sourceLabel: '聊天图片' });
          }
        } catch (error) {
          if (!controller.signal.aborted) notification.addNotification({ type: 'error', title: '图片预览失败', message: String(error), toastOnly: true });
        }
        return;
      }
    }
  };

  const handleDoubleClick = (e: MouseEvent) => {
    if (e.target instanceof Element && e.target.closest(MERMAID_CONTENT_SELECTOR)) {
      e.stopPropagation();
    }
  };

  onMounted(() => {
    eventContainer = containerRef.value;
    eventContainer?.addEventListener("click", handleClick);
    eventContainer?.addEventListener("dblclick", handleDoubleClick);
  });

  onUnmounted(() => {
    imageController?.abort();
    eventContainer?.removeEventListener("click", handleClick);
    eventContainer?.removeEventListener("dblclick", handleDoubleClick);
    eventContainer = null;
  });
}
