const REVEAL_FRAME_INTERVAL_MS = 1000 / 30;
const REVEAL_DRAIN_STEPS = 3;
const REVEAL_HARD_LAG_MS = 200;
const MAX_REVEAL_BATCH_CODE_UNITS = 256;
const MAX_REVEAL_DEBT_UNITS = 64;
const MAX_REVEAL_UNITS_PER_TICK = 64;

export interface StreamRevealBatch<T> {
  targetId: string;
  text: string;
  metadata: T;
}

interface StreamRevealCallbacks<T> {
  apply: (targetId: string, text: string) => boolean;
  complete: (metadata: T) => void;
  fail: (metadata: T, reason: string) => void;
}

export interface StreamRevealController<T> {
  enqueue: (batch: StreamRevealBatch<T>) => void;
  flush: () => boolean;
  cancel: () => void;
  dispose: () => void;
  readonly hasPending: boolean;
}

interface PendingRevealBatch {
  targetId: string;
  units: string[];
  offset: number;
  metadata: unknown;
  enqueuedAt: number;
}

interface RevealState {
  batches: PendingRevealBatch[];
  pendingUnits: number;
  callbacks: StreamRevealCallbacks<unknown>;
  disposed: boolean;
}

const revealStates = new Set<RevealState>();
let wakeTimerId: number | null = null;
let animationFrameId: number | null = null;

const now = (): number =>
  typeof performance !== "undefined" ? performance.now() : Date.now();

type GraphemeSegmenter = {
  segment: (value: string) => Iterable<{ segment: string }>;
};
let graphemeSegmenter: GraphemeSegmenter | null | undefined;

const COMBINING_MARK = /^\p{Mark}$/u;
const REGIONAL_INDICATOR = /^[\u{1F1E6}-\u{1F1FF}]$/u;

function isGraphemeExtender(value: string): boolean {
  const codePoint = value.codePointAt(0) ?? 0;
  return (
    COMBINING_MARK.test(value) ||
    codePoint === 0xfe0e ||
    codePoint === 0xfe0f ||
    (codePoint >= 0x1f3fb && codePoint <= 0x1f3ff) ||
    (codePoint >= 0xe0020 && codePoint <= 0xe007f)
  );
}

function splitFallbackGraphemes(text: string): string[] {
  const result: string[] = [];
  let current = "";

  for (const codePoint of Array.from(text)) {
    const joinsPrevious =
      current.endsWith("\u200d") ||
      codePoint === "\u200d" ||
      isGraphemeExtender(codePoint) ||
      (REGIONAL_INDICATOR.test(codePoint) && REGIONAL_INDICATOR.test(current));
    if (!current || joinsPrevious) {
      current += codePoint;
    } else {
      result.push(current);
      current = codePoint;
    }
  }

  if (current) result.push(current);
  return result;
}

function splitGraphemes(text: string): string[] {
  const Segmenter = (
    Intl as typeof Intl & {
      Segmenter?: new (
        locale?: string | string[],
        options?: { granularity: "grapheme" },
      ) => GraphemeSegmenter;
    }
  ).Segmenter;
  if (Segmenter) {
    if (graphemeSegmenter === undefined || graphemeSegmenter === null) {
      graphemeSegmenter = new Segmenter(undefined, { granularity: "grapheme" });
    }
    return Array.from(
      graphemeSegmenter.segment(text),
      (entry) => entry.segment,
    );
  }
  return splitFallbackGraphemes(text);
}

export function canSmoothStreamAppend(text: string): boolean {
  return (
    text.length <= MAX_REVEAL_BATCH_CODE_UNITS &&
    splitGraphemes(text).length > 1
  );
}

function hasPendingReveal(): boolean {
  for (const state of revealStates) {
    if (!state.disposed && state.pendingUnits > 0) return true;
  }
  return false;
}

function cancelGlobalScheduleIfIdle(): void {
  if (hasPendingReveal()) return;
  if (wakeTimerId !== null) {
    window.clearTimeout(wakeTimerId);
    wakeTimerId = null;
  }
  if (animationFrameId !== null) {
    cancelAnimationFrame(animationFrameId);
    animationFrameId = null;
  }
}

function releaseStateIfIdle(state: RevealState): void {
  if (state.pendingUnits <= 0) revealStates.delete(state);
}

function failState(
  state: RevealState,
  metadata: unknown,
  reason: string,
): false {
  state.batches = [];
  state.pendingUnits = 0;
  releaseStateIfIdle(state);
  state.callbacks.fail(metadata, reason);
  cancelGlobalScheduleIfIdle();
  return false;
}

function flushState(state: RevealState): boolean {
  if (state.disposed) return false;

  while (state.batches.length > 0) {
    const batch = state.batches[0];
    const remaining = batch.units.slice(batch.offset).join("");
    if (remaining && !state.callbacks.apply(batch.targetId, remaining)) {
      return failState(state, batch.metadata, "append target is unavailable");
    }
    state.pendingUnits -= batch.units.length - batch.offset;
    state.batches.shift();
    state.callbacks.complete(batch.metadata);
  }

  state.pendingUnits = 0;
  releaseStateIfIdle(state);
  cancelGlobalScheduleIfIdle();
  return true;
}

function consumeState(state: RevealState, currentTime: number): boolean {
  const first = state.batches[0];
  if (!first || state.pendingUnits <= 0) return true;
  if (currentTime - first.enqueuedAt >= REVEAL_HARD_LAG_MS) {
    return flushState(state);
  }

  let budget = Math.ceil(state.pendingUnits / REVEAL_DRAIN_STEPS);
  const catchUpBudget = Math.max(0, state.pendingUnits - MAX_REVEAL_DEBT_UNITS);
  budget = Math.max(
    1,
    catchUpBudget,
    Math.min(MAX_REVEAL_UNITS_PER_TICK, budget),
  );

  while (budget > 0 && state.batches.length > 0) {
    const batch = state.batches[0];
    const remainingUnits = batch.units.length - batch.offset;
    const take = Math.min(budget, remainingUnits);
    const slice = batch.units.slice(batch.offset, batch.offset + take).join("");
    if (slice && !state.callbacks.apply(batch.targetId, slice)) {
      return failState(state, batch.metadata, "append target is unavailable");
    }

    batch.offset += take;
    state.pendingUnits -= take;
    budget -= take;
    if (batch.offset === batch.units.length) {
      state.batches.shift();
      state.callbacks.complete(batch.metadata);
    }
  }

  releaseStateIfIdle(state);

  return true;
}

function requestRevealTick(): void {
  if (
    wakeTimerId !== null ||
    animationFrameId !== null ||
    !hasPendingReveal()
  ) {
    return;
  }

  wakeTimerId = window.setTimeout(() => {
    wakeTimerId = null;
    if (!hasPendingReveal()) return;
    animationFrameId = requestAnimationFrame(() => {
      animationFrameId = null;
      const currentTime = now();
      for (const state of revealStates) {
        if (!state.disposed && state.pendingUnits > 0) {
          consumeState(state, currentTime);
        }
      }
      requestRevealTick();
    });
  }, REVEAL_FRAME_INTERVAL_MS);
}

export function createStreamRevealController<T>(
  callbacks: StreamRevealCallbacks<T>,
): StreamRevealController<T> {
  const state: RevealState = {
    batches: [],
    pendingUnits: 0,
    callbacks: callbacks as StreamRevealCallbacks<unknown>,
    disposed: false,
  };

  return {
    enqueue(batch) {
      if (state.disposed || !batch.text) return;
      const units = splitGraphemes(batch.text);
      if (units.length === 0) {
        callbacks.complete(batch.metadata);
        return;
      }

      const wasIdle = state.pendingUnits === 0;
      revealStates.add(state);
      state.batches.push({
        targetId: batch.targetId,
        units,
        offset: 0,
        metadata: batch.metadata,
        enqueuedAt: now(),
      });
      state.pendingUnits += units.length;

      // 首批正文在当前 Vue post-flush 中先展示一小段，避免平滑模式额外制造首字延迟。
      if (wasIdle || state.pendingUnits > MAX_REVEAL_DEBT_UNITS) {
        consumeState(state, now());
      }
      requestRevealTick();
    },
    flush: () => flushState(state),
    cancel() {
      state.batches = [];
      state.pendingUnits = 0;
      revealStates.delete(state);
      cancelGlobalScheduleIfIdle();
    },
    dispose() {
      if (state.disposed) return;
      state.disposed = true;
      state.batches = [];
      state.pendingUnits = 0;
      revealStates.delete(state);
      cancelGlobalScheduleIfIdle();
    },
    get hasPending() {
      return state.pendingUnits > 0;
    },
  };
}

const flushRevealDebtWhenHidden = () => {
  if (!document.hidden) return;
  for (const state of [...revealStates]) flushState(state);
};
document.addEventListener("visibilitychange", flushRevealDebtWhenHidden);

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    document.removeEventListener("visibilitychange", flushRevealDebtWhenHidden);
    for (const state of [...revealStates]) {
      state.batches = [];
      state.pendingUnits = 0;
      state.disposed = true;
    }
    revealStates.clear();
    cancelGlobalScheduleIfIdle();
  });
}
