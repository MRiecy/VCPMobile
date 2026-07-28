<script setup lang="ts">
defineOptions({
  inheritAttrs: false,
});

import { nextTick, onBeforeUnmount, ref, useId, watch } from "vue";
import { AlertTriangle, Info } from "lucide-vue-next";

const props = defineProps<{
  title: string;
  message: string;
  isOpen: boolean;
  confirmText?: string;
  cancelText?: string;
  isDanger?: boolean;
  onlyConfirm?: boolean;
}>();

const emit = defineEmits<{
  (event: "confirm"): void;
  (event: "cancel"): void;
}>();

const instanceId = useId();
const titleId = `${instanceId}-title`;
const messageId = `${instanceId}-message`;
const dialogRef = ref<HTMLElement | null>(null);
const cancelButtonRef = ref<HTMLButtonElement | null>(null);
const confirmButtonRef = ref<HTMLButtonElement | null>(null);

let previousFocus: HTMLElement | null = null;
let decisionEmitted = false;

const restoreFocus = () => {
  if (previousFocus?.isConnected) {
    previousFocus.focus({ preventScroll: true });
  }
  previousFocus = null;
};

const focusInitialControl = () => {
  const target = props.onlyConfirm
    ? confirmButtonRef.value
    : cancelButtonRef.value || confirmButtonRef.value;
  (target || dialogRef.value)?.focus({ preventScroll: true });
};

watch(
  () => props.isOpen,
  async (isOpen) => {
    if (isOpen) {
      decisionEmitted = false;
      previousFocus =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      await nextTick();
      focusInitialControl();
      return;
    }

    restoreFocus();
  },
  { immediate: true },
);

const emitDecision = (confirmed: boolean) => {
  if (decisionEmitted) return;
  decisionEmitted = true;
  if (confirmed) {
    emit("confirm");
  } else {
    emit("cancel");
  }
};

const handleConfirm = () => {
  emitDecision(true);
};

const handleDismiss = () => {
  emitDecision(props.onlyConfirm ? true : false);
};

const getFocusableControls = (): HTMLElement[] => {
  if (!dialogRef.value) return [];

  return Array.from(
    dialogRef.value.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  );
};

const handleKeydown = (event: KeyboardEvent) => {
  if (event.key === "Escape") {
    event.preventDefault();
    handleDismiss();
    return;
  }

  if (event.key !== "Tab") return;

  const controls = getFocusableControls();
  if (controls.length === 0) {
    event.preventDefault();
    dialogRef.value?.focus({ preventScroll: true });
    return;
  }

  const first = controls[0];
  const last = controls[controls.length - 1];
  const activeElement = document.activeElement;

  if (event.shiftKey && activeElement === first) {
    event.preventDefault();
    last.focus({ preventScroll: true });
  } else if (!event.shiftKey && activeElement === last) {
    event.preventDefault();
    first.focus({ preventScroll: true });
  }
};

onBeforeUnmount(restoreFocus);
</script>

<template>
  <Teleport to="body">
    <Transition name="vcp-confirm-fade">
      <div
        v-if="isOpen"
        v-bind="$attrs"
        class="fixed inset-0 z-dialog flex items-center justify-center bg-black/55 px-5 py-[max(24px,var(--vcp-safe-top,24px))] backdrop-blur-sm pointer-events-auto"
        @click.self="handleDismiss"
        @keydown="handleKeydown"
      >
        <section
          ref="dialogRef"
          class="vcp-confirm-modal w-full max-w-sm overflow-hidden rounded-3xl border border-white/10 p-5 shadow-2xl outline-none"
          :role="isDanger || onlyConfirm ? 'alertdialog' : 'dialog'"
          aria-modal="true"
          :aria-labelledby="titleId"
          :aria-describedby="messageId"
          tabindex="-1"
        >
          <div class="flex items-start gap-3.5">
            <div
              class="mt-0.5 flex h-10 w-10 shrink-0 items-center justify-center rounded-2xl"
              :class="
                isDanger
                  ? 'bg-danger/12 text-danger'
                  : 'bg-blue-500/12 text-blue-500'
              "
              aria-hidden="true"
            >
              <AlertTriangle v-if="isDanger" :size="20" />
              <Info v-else :size="20" />
            </div>

            <div class="min-w-0 flex-1">
              <h2
                :id="titleId"
                class="text-base font-black leading-6 text-primary-text"
              >
                {{ title }}
              </h2>
              <p
                :id="messageId"
                class="mt-1.5 whitespace-pre-wrap break-words text-sm leading-6 text-secondary-text"
              >
                {{ message }}
              </p>
            </div>
          </div>

          <div class="mt-6 flex justify-end gap-2.5">
            <button
              v-if="!onlyConfirm"
              ref="cancelButtonRef"
              type="button"
              class="min-h-11 rounded-xl border border-black/10 bg-black/5 px-5 text-sm font-bold text-primary-text transition-colors hover:bg-black/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60 dark:border-white/10 dark:bg-white/5 dark:hover:bg-white/10"
              @click="handleDismiss"
            >
              {{ cancelText || "取消" }}
            </button>
            <button
              ref="confirmButtonRef"
              type="button"
              class="min-h-11 rounded-xl px-5 text-sm font-bold text-white shadow-lg transition-[transform,background-color,opacity] active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--secondary-bg)]"
              :class="
                isDanger
                  ? 'bg-danger shadow-danger/20 hover:bg-danger/90 focus-visible:ring-danger/70'
                  : 'bg-blue-500 shadow-blue-500/20 hover:bg-blue-600 focus-visible:ring-blue-500/70'
              "
              @click="handleConfirm"
            >
              {{ confirmText || "确认" }}
            </button>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.vcp-confirm-modal {
  color: var(--primary-text);
  background: linear-gradient(
    145deg,
    color-mix(in srgb, var(--secondary-bg) 96%, white 4%),
    var(--secondary-bg)
  );
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.05),
    0 24px 70px rgba(0, 0, 0, 0.42);
}

.vcp-confirm-fade-enter-active,
.vcp-confirm-fade-leave-active {
  transition: opacity 180ms ease;
}

.vcp-confirm-fade-enter-active .vcp-confirm-modal,
.vcp-confirm-fade-leave-active .vcp-confirm-modal {
  transition:
    transform 200ms cubic-bezier(0.22, 1, 0.36, 1),
    opacity 160ms ease;
}

.vcp-confirm-fade-enter-from,
.vcp-confirm-fade-leave-to,
.vcp-confirm-fade-enter-from .vcp-confirm-modal,
.vcp-confirm-fade-leave-to .vcp-confirm-modal {
  opacity: 0;
}

.vcp-confirm-fade-enter-from .vcp-confirm-modal,
.vcp-confirm-fade-leave-to .vcp-confirm-modal {
  transform: translateY(10px) scale(0.97);
}

@media (prefers-reduced-motion: reduce) {
  .vcp-confirm-fade-enter-active,
  .vcp-confirm-fade-leave-active,
  .vcp-confirm-fade-enter-active .vcp-confirm-modal,
  .vcp-confirm-fade-leave-active .vcp-confirm-modal {
    transition-duration: 1ms;
  }
}
</style>
