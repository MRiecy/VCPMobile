import { computed, onUnmounted, ref, watch } from "vue";
import morphdom from "morphdom";
import type { ChatMessage, ContentBlock, TailFrame } from "../../../core/types/chat";
import { applyFrame, cleanupRegistry, rebuildSnapshot } from "../../../core/utils/astExecutor";
import { applyThoughtTailOp, cleanupThoughtTail } from "../../../core/dom-engine";
import { clearStreamElementReveals, flushStreamTextFragments } from "../../../core/utils/streamTextFade";

interface MessageTailRendererOptions {
  getMessage: () => ChatMessage;
  /** 读取消息投影已经剥离 style 的尾部 HTML；CSS 由消息级入口提交。 */
  renderTailHtml: (block: ContentBlock) => string;
  isInlineHtmlBlock: (type: string) => boolean;
  shouldAnimateFrame: (frame: TailFrame) => boolean;
  /** 没有可恢复的会话时返回 null，不切换当前 DOM 的执行模式。 */
  requestSnapshot: (messageId: string, reason: string) => Promise<boolean> | null;
}

function isAstDebugEnabled(): boolean {
  return Boolean(import.meta.env.DEV && (window as any).__VCP_AST_DEBUG__);
}

/**
 * 随整条消息存活的尾部 DOM owner。气泡拆分只更换 sandbox，不能重置恢复与降级历史。
 * Store 的接收帧水位与这里的 DOM 已应用帧水位分别维护。
 */
export function useMessageTailRenderer(options: MessageTailRendererOptions) {
  const tailSandboxRef = ref<HTMLElement | null>(null);
  const tailRootRef = ref<HTMLElement | null>(null);
  const thoughtTailSandboxRef = ref<HTMLElement | null>(null);
  const enableAstDiff = ref(true);
  const isAstRecoveryPending = ref(false);
  let astRecoveryPromise: Promise<boolean> | null = null;
  let lastAppliedFrameSeq = 0;
  let localTailStreamId = -1;
  let localTailEpoch = -1;
  let astFailureCount = 0;
  let lastSandbox: HTMLElement | null = null;
  let lastThoughtSandbox: HTMLElement | null = null;
  let disposed = false;

  const isPlainTailFallback = computed(() => {
    const block = options.getMessage().tailBlock;
    return !!block && (
      block.render_mode === "plain"
      || block.render_mode === undefined && (!block.nodes || block.nodes.length === 0)
    );
  });
  const thoughtTailBlock = computed(() => {
    const block = options.getMessage().tailBlock;
    return block?.type === "thought" ? block : null;
  });
  const useAstForCurrentTail = computed(() => {
    if (!enableAstDiff.value || isAstRecoveryPending.value || isPlainTailFallback.value) return false;
    const message = options.getMessage();
    return !!message.tailFrame || !!message.tailBlock?.nodes || !!message.tailSnapshot;
  });

  function requestCurrentTailSnapshot(reason: string): Promise<boolean> {
    if (astRecoveryPromise) return astRecoveryPromise;
    const recovery = options.requestSnapshot(options.getMessage().id, reason);
    if (!recovery) return Promise.resolve(false);

    isAstRecoveryPending.value = true;
    const completion = recovery.finally(() => {
      if (disposed || astRecoveryPromise !== completion) return;
      isAstRecoveryPending.value = false;
      astRecoveryPromise = null;
    });
    astRecoveryPromise = completion;
    return completion;
  }

  function recoverTailSnapshotOrDowngrade(reason: string): void {
    if (disposed || !enableAstDiff.value) return;
    astFailureCount += 1;
    if (isAstDebugEnabled()) {
      console.warn(`[AST Diff Recovery] ${options.getMessage().id}: ${reason}. failureCount=${astFailureCount}`);
    }
    void requestCurrentTailSnapshot(reason).then((recovered) => {
      if (disposed) return;
      if (recovered) {
        astFailureCount = 0;
      } else if (astFailureCount >= 2) {
        enableAstDiff.value = false;
        cleanupRegistry(options.getMessage().id);
      }
    });
  }

  function markTailFrameApplied(frame: TailFrame): void {
    lastAppliedFrameSeq = frame.frameSeq;
    localTailStreamId = frame.streamId;
    localTailEpoch = frame.epoch;
    astFailureCount = 0;
  }

  function finishTailMotion(contentRoot?: HTMLElement | null): void {
    flushStreamTextFragments(options.getMessage().id);
    const root = contentRoot ?? lastSandbox;
    if (root) clearStreamElementReveals(root);
  }

  const fallbackTailSignature = computed(() => {
    const block = options.getMessage().tailBlock;
    if (!block) return "";
    const contentSignal = block.hash !== undefined && block.hash !== null
      ? String(block.hash)
      : block.content || "";
    return [block.type, contentSignal, block.nodes?.length ?? 0].join("|");
  });

  watch(
    [fallbackTailSignature, useAstForCurrentTail, tailRootRef],
    () => {
      const block = options.getMessage().tailBlock;
      const root = tailRootRef.value;
      if (useAstForCurrentTail.value || isPlainTailFallback.value) return;
      if (!root || !block || !options.isInlineHtmlBlock(block.type)) return;

      // post watcher 已在 Vue 更新 DOM 后执行；在同一轮读取并提交，避免延迟闭包写入新根。
      const html = options.renderTailHtml(block);
      try {
        morphdom(root, `<div>${html}</div>`, {
          childrenOnly: true,
          getNodeKey: (node) => {
            if (!node || node.nodeType !== 1) return undefined;
            const element = node as Element;
            return element.id || element.getAttribute("data-vcp-key") || undefined;
          },
          onBeforeElUpdated: (fromEl, toEl) => {
            if (fromEl.isEqualNode(toEl)) return false;

            if (fromEl.tagName === "VIDEO" || fromEl.tagName === "AUDIO") {
              if (!(fromEl as HTMLMediaElement).paused) return false;
            }
            if (fromEl === document.activeElement) {
              requestAnimationFrame(() => {
                if (toEl && typeof toEl.focus === "function") toEl.focus();
              });
            }
            if (fromEl.tagName === "IMG") {
              const fromImg = fromEl as HTMLImageElement;
              const toImg = toEl as HTMLImageElement;
              if (fromImg.onerror && !toImg.onerror) toImg.onerror = fromImg.onerror;
              if (fromImg.onload && !toImg.onload) toImg.onload = fromImg.onload;
              if (fromImg.style.visibility) toImg.style.visibility = fromImg.style.visibility;
              if (fromImg.complete && fromImg.naturalWidth > 0) return false;
            }
            return true;
          },
        });
      } catch (error) {
        console.debug("[TailMorphdom] Skipped frame:", error);
      }
    },
    { immediate: true, flush: "post" },
  );

  watch(
    [() => options.getMessage().tailFrame, () => options.getMessage().tailSnapshot, tailSandboxRef],
    ([frame, _snapshot, sandbox]) => {
      const message = options.getMessage();
      const debugEnabled = isAstDebugEnabled();
      if (debugEnabled) {
        console.warn(`[AST Diff Watch] Msg ${message.id} stream=${frame?.streamId ?? "none"}, frame=${frame?.frameSeq ?? "none"}, mutations=${frame?.mutations?.length || 0}, sandbox=${sandbox ? "Ready" : "Null"}, epoch=${frame?.epoch}, revision=${frame?.revision}`);
      }

      if (!useAstForCurrentTail.value || !sandbox) {
        if (lastSandbox) {
          cleanupRegistry(message.id);
          lastSandbox.innerHTML = "";
          lastSandbox = null;
        }
        return;
      }

      if (lastSandbox !== sandbox) {
        cleanupRegistry(message.id);
        sandbox.innerHTML = "";
        lastAppliedFrameSeq = 0;
        localTailStreamId = -1;
        localTailEpoch = -1;
        lastSandbox = sandbox;
        if (frame?.reset !== true && message.tailBlock?.nodes) {
          rebuildSnapshot(message.tailBlock.nodes, message.id, sandbox);
          astFailureCount = 0;
          // 重建使用当前完整 AST，认领当前帧，避免再次重建或重复执行同一批 mutation。
          if (frame) markTailFrameApplied(frame);
        } else if (frame && frame.frameSeq > 1 && !(frame.reset === true && frame.snapshot !== undefined)) {
          recoverTailSnapshotOrDowngrade("sandbox_remount");
          return;
        }
      }

      if (!frame) return;
      const incomingStreamId = frame.streamId ?? 0;
      const incomingEpoch = frame.epoch ?? 0;
      const explicitReset = frame.reset === true
        || incomingStreamId !== localTailStreamId
        || incomingEpoch !== localTailEpoch;
      let startsFromEmpty = false;

      // frameSeq 只在同一 stream/epoch 内可比，新流必须先接管身份。
      if (!explicitReset && frame.frameSeq <= lastAppliedFrameSeq) return;

      if (explicitReset) {
        const snapshot = frame.snapshot ?? message.tailBlock?.nodes;
        const canStartFromEmpty = frame.reset !== true && frame.frameSeq === 1 && lastAppliedFrameSeq === 0;
        if (snapshot === undefined && !canStartFromEmpty) {
          recoverTailSnapshotOrDowngrade("reset_without_snapshot");
          return;
        }
        sandbox.innerHTML = "";
        cleanupRegistry(message.id);
        localTailStreamId = incomingStreamId;
        localTailEpoch = incomingEpoch;
        lastAppliedFrameSeq = 0;
        if (snapshot !== undefined) rebuildSnapshot(snapshot, message.id, sandbox);
        startsFromEmpty = snapshot === undefined && canStartFromEmpty;
      }

      const mutations = frame.mutations || [];
      if (mutations.length === 0) {
        markTailFrameApplied(frame);
        return;
      }
      if (debugEnabled) {
        console.warn(`[AST Diff Apply] Executing frame ${frame.frameSeq} (${mutations.length} mutations) for ${message.id}`);
      }
      const result = applyFrame(mutations, message.id, sandbox, {
        smoothStreaming: options.shouldAnimateFrame(frame) && (!explicitReset || startsFromEmpty),
      });
      if (result.ok) {
        markTailFrameApplied(frame);
      } else {
        recoverTailSnapshotOrDowngrade(result.failed?.reason || "applyFrame failed");
      }
    },
    { flush: "post", immediate: true },
  );

  watch(
    [() => options.getMessage().tailOp, () => options.getMessage().tailBlock?.content, thoughtTailSandboxRef],
    ([op, fallbackContent, sandbox]) => {
      const messageId = options.getMessage().id;
      if (!sandbox || !thoughtTailBlock.value) {
        if (lastThoughtSandbox) {
          cleanupThoughtTail(messageId);
          lastThoughtSandbox = null;
        }
        return;
      }
      if (lastThoughtSandbox !== sandbox) {
        cleanupThoughtTail(messageId);
        lastThoughtSandbox = sandbox;
      }
      applyThoughtTailOp(messageId, sandbox, op, fallbackContent);
    },
    { flush: "post", immediate: true },
  );

  onUnmounted(() => {
    disposed = true;
    cleanupRegistry(options.getMessage().id);
    cleanupThoughtTail(options.getMessage().id);
  });

  return {
    tailSandboxRef,
    tailRootRef,
    thoughtTailSandboxRef,
    isPlainTailFallback,
    thoughtTailBlock,
    useAstForCurrentTail,
    isAstRecoveryPending,
    finishTailMotion,
  };
}
