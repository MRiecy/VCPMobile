export const STREAM_REVEAL_DURATION_MS = 250;
export const STREAM_INLINE_REVEAL_CLASS = "vcp-stream-inline-reveal";
export const STREAM_ELEMENT_REVEAL_CLASS = "vcp-stream-element-reveal";

const STREAM_REVEAL_PROGRESS = "--vcp-stream-reveal-progress";
const STREAM_REVEAL_END = "calc(100% + 1.732em)";
const STREAM_REVEAL_MASK = "linear-gradient(120deg, #000 calc(var(--vcp-stream-reveal-progress) - 1.732em), transparent var(--vcp-stream-reveal-progress))";

let revealProgressRegistered = false;

function ensureRevealProgressRegistered(): boolean {
  if (revealProgressRegistered) return true;
  if (typeof CSS === "undefined" || typeof CSS.registerProperty !== "function") {
    return false;
  }

  try {
    CSS.registerProperty({
      name: STREAM_REVEAL_PROGRESS,
      syntax: "<length-percentage>",
      inherits: false,
      initialValue: "0%",
    });
    revealProgressRegistered = true;
  } catch (error) {
    // HMR 或重复 bundle 可能已注册同名属性；这仍表示 typed interpolation 可用。
    if ((error as { name?: string }).name !== "InvalidModificationError") {
      return false;
    }
    revealProgressRegistered = true;
  }
  return true;
}

export function supportsStreamRevealMotion(): boolean {
  if (
    typeof Element === "undefined"
    || typeof Element.prototype.animate !== "function"
    || typeof CSS === "undefined"
    || typeof CSS.supports !== "function"
  ) {
    return false;
  }

  const supportsMask = CSS.supports("mask-image", STREAM_REVEAL_MASK)
    || CSS.supports("-webkit-mask-image", STREAM_REVEAL_MASK);
  return supportsMask && ensureRevealProgressRegistered();
}

interface StreamTextFragment {
  chunk: string;
  element: HTMLSpanElement;
  animation: Animation;
  frameToken: object;
  done: boolean;
  finish: () => void;
}

interface StreamTextTarget {
  messageId: string;
  targetId: string;
  textNode: Text;
  fragments: StreamTextFragment[];
}

const fragmentsByMessage = new Map<string, Map<string, StreamTextTarget>>();

function stopFragmentAnimation(fragment: StreamTextFragment): void {
  fragment.animation.onfinish = null;
  fragment.animation.oncancel = null;
  if (fragment.animation.playState !== "idle") {
    fragment.animation.cancel();
  }
}

function releaseTargetIfEmpty(state: StreamTextTarget): void {
  if (state.fragments.length > 0) return;
  const messageTargets = fragmentsByMessage.get(state.messageId);
  if (!messageTargets || messageTargets.get(state.targetId) !== state) return;
  messageTargets.delete(state.targetId);
  if (messageTargets.size === 0) fragmentsByMessage.delete(state.messageId);
}

function drainCompletedFragments(state: StreamTextTarget): void {
  while (state.fragments[0]?.done) {
    const fragment = state.fragments.shift();
    if (!fragment) break;
    stopFragmentAnimation(fragment);
    state.textNode.appendData(fragment.chunk);
    fragment.element.remove();
  }
  releaseTargetIfEmpty(state);
}

function settleTarget(state: StreamTextTarget, commit: boolean): void {
  const messageTargets = fragmentsByMessage.get(state.messageId);
  if (messageTargets?.get(state.targetId) === state) {
    messageTargets.delete(state.targetId);
    if (messageTargets.size === 0) fragmentsByMessage.delete(state.messageId);
  }

  for (const fragment of state.fragments) {
    stopFragmentAnimation(fragment);
    if (commit) state.textNode.appendData(fragment.chunk);
    fragment.element.remove();
  }
  state.fragments.length = 0;
}

export function appendStreamTextFragment(
  messageId: string,
  targetId: string,
  textNode: Text,
  chunk: string,
  frameToken: object,
): void {
  if (!chunk) return;
  if (textNode.parentElement?.closest(`.${STREAM_ELEMENT_REVEAL_CLASS}`)) {
    textNode.appendData(chunk);
    return;
  }

  let messageTargets = fragmentsByMessage.get(messageId);
  if (!messageTargets) {
    messageTargets = new Map();
    fragmentsByMessage.set(messageId, messageTargets);
  }

  let state = messageTargets.get(targetId);
  if (state && state.textNode !== textNode) {
    settleTarget(state, true);
    messageTargets = fragmentsByMessage.get(messageId) ?? new Map();
    fragmentsByMessage.set(messageId, messageTargets);
    state = undefined;
  }

  if (!state) {
    state = { messageId, targetId, textNode, fragments: [] };
    messageTargets.set(targetId, state);
  }

  const parent = textNode.parentNode;
  const lastFragment = state.fragments[state.fragments.length - 1];
  const anchor = lastFragment?.element ?? textNode;
  if (!parent || anchor.parentNode !== parent) {
    settleTarget(state, true);
    textNode.appendData(chunk);
    return;
  }

  // 同一 applyFrame / Text target 的连续 append 是同一个视觉 surface。
  if (
    lastFragment
    && lastFragment.frameToken === frameToken
    && !lastFragment.done
    && lastFragment.element.parentNode === parent
  ) {
    lastFragment.chunk += chunk;
    lastFragment.element.append(chunk);
    return;
  }

  if (!supportsStreamRevealMotion()) {
    settleTarget(state, true);
    textNode.appendData(chunk);
    return;
  }

  const element = document.createElement("span");
  element.className = STREAM_INLINE_REVEAL_CLASS;
  element.dataset.vcpStreamFragment = "";
  element.textContent = chunk;
  parent.insertBefore(element, anchor.nextSibling);

  let animation: Animation;
  try {
    animation = element.animate(
      [
        { [STREAM_REVEAL_PROGRESS]: "0%" },
        { [STREAM_REVEAL_PROGRESS]: STREAM_REVEAL_END },
      ] as Keyframe[],
      {
        duration: STREAM_REVEAL_DURATION_MS,
        easing: "linear",
        fill: "both",
      },
    );
  } catch {
    element.remove();
    settleTarget(state, true);
    textNode.appendData(chunk);
    return;
  }

  const fragment: StreamTextFragment = {
    chunk,
    element,
    animation,
    frameToken,
    done: false,
    finish: () => {
      if (!state?.fragments.includes(fragment)) return;
      fragment.done = true;
      drainCompletedFragments(state);
    },
  };
  animation.onfinish = fragment.finish;
  animation.oncancel = fragment.finish;
  state.fragments.push(fragment);
}

export function clearStreamElementReveals(root: ParentNode = document): void {
  root
    .querySelectorAll(`.${STREAM_ELEMENT_REVEAL_CLASS}`)
    .forEach((element) => element.classList.remove(STREAM_ELEMENT_REVEAL_CLASS));
}

export function flushStreamTextFragments(messageId: string): void {
  const messageTargets = fragmentsByMessage.get(messageId);
  if (!messageTargets) return;
  for (const state of [...messageTargets.values()]) settleTarget(state, true);
}

export function discardStreamTextFragments(messageId: string): void {
  const messageTargets = fragmentsByMessage.get(messageId);
  if (!messageTargets) return;
  for (const state of [...messageTargets.values()]) settleTarget(state, false);
}

function finishAllStreamRevealMotion(): void {
  for (const messageId of [...fragmentsByMessage.keys()]) {
    flushStreamTextFragments(messageId);
  }
  if (typeof document !== "undefined") clearStreamElementReveals(document);
}

const reducedMotionQuery = typeof window !== "undefined"
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : null;
export function prefersReducedStreamMotion(): boolean {
  return reducedMotionQuery?.matches === true;
}
const handleReducedMotion = (event: MediaQueryListEvent) => {
  if (event.matches) finishAllStreamRevealMotion();
};
const handleVisibilityChange = () => {
  if (document.hidden) finishAllStreamRevealMotion();
};

reducedMotionQuery?.addEventListener("change", handleReducedMotion);
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", handleVisibilityChange);
}

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    reducedMotionQuery?.removeEventListener("change", handleReducedMotion);
    document.removeEventListener("visibilitychange", handleVisibilityChange);
    finishAllStreamRevealMotion();
  });
}
