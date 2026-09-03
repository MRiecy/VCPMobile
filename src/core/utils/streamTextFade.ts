export interface StreamTextFadeStyle {
  durationMs: number;
  fromOpacity: number;
}

interface StreamTextFragment {
  chunk: string;
  element: HTMLSpanElement;
  animation: Animation;
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

export function resolveStreamTextFade(codeUnits: number): StreamTextFadeStyle {
  const progress = Math.min(1, Math.max(0, Math.log2(Math.max(1, codeUnits)) / 6));
  return {
    durationMs: Math.round(110 - 50 * progress),
    fromOpacity: 0.8 + 0.14 * progress,
  };
}

function stopFragmentAnimation(fragment: StreamTextFragment): void {
  fragment.animation.onfinish = null;
  fragment.animation.oncancel = null;
  if (fragment.animation.playState !== "finished") {
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
  fade: StreamTextFadeStyle,
): void {
  if (!chunk) return;
  if (textNode.parentElement?.closest(".vcp-stream-element-fade-in")) {
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

  const element = document.createElement("span");
  element.className = "vcp-stream-inline-fade";
  element.dataset.vcpStreamFragment = "";
  element.style.setProperty("--vcp-stream-fade-duration", `${fade.durationMs}ms`);
  element.style.setProperty("--vcp-stream-fade-from", fade.fromOpacity.toFixed(3));
  element.textContent = chunk;

  if (typeof element.animate !== "function") {
    settleTarget(state, true);
    textNode.appendData(chunk);
    return;
  }

  let animation: Animation;
  try {
    animation = element.animate(
      [{ opacity: fade.fromOpacity }, { opacity: 1 }],
      {
        duration: fade.durationMs,
        easing: "cubic-bezier(0.2, 0, 0, 1)",
        fill: "backwards",
      },
    );
  } catch {
    settleTarget(state, true);
    textNode.appendData(chunk);
    return;
  }

  const fragment: StreamTextFragment = {
    chunk,
    element,
    animation,
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
  parent.insertBefore(element, anchor.nextSibling);
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

function flushAllStreamTextFragments(): void {
  for (const messageId of [...fragmentsByMessage.keys()]) {
    flushStreamTextFragments(messageId);
  }
}

const reducedMotionQuery = typeof window !== "undefined"
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : null;
export function prefersReducedStreamMotion(): boolean {
  return reducedMotionQuery?.matches === true;
}
const handleReducedMotion = (event: MediaQueryListEvent) => {
  if (event.matches) flushAllStreamTextFragments();
};
const handleVisibilityChange = () => {
  if (document.hidden) flushAllStreamTextFragments();
};

reducedMotionQuery?.addEventListener("change", handleReducedMotion);
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", handleVisibilityChange);
}

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    reducedMotionQuery?.removeEventListener("change", handleReducedMotion);
    document.removeEventListener("visibilitychange", handleVisibilityChange);
    flushAllStreamTextFragments();
  });
}
