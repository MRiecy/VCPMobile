<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, useId, watch } from 'vue';
import { onClickOutside, useDocumentVisibility } from '@vueuse/core';
import { AudioLines, Check, ChevronDown } from '@lucide/vue';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useVoiceCallStore } from './voiceCallStore';
import type { VoiceInputDevices } from './voiceConversation';

const call = useVoiceCallStore();
const { registerModal, unregisterModal } = useModalHistory();
const panelId = `voice-inputs-${useId()}`;
const open = ref(false);
const trigger = ref<HTMLButtonElement | null>(null);
const bounds = ref<HTMLElement | null>(null);
const panel = ref<HTMLElement | null>(null);
const inputs = shallowRef<VoiceInputDevices | null>(null);
const selecting = ref(false);
const readError = ref(false);
const selectionError = ref('');
const refresh = ref(0);
const visibility = useDocumentVisibility();
let visit = 0;

const close = () => { open.value = false; };
const toggle = () => {
  if (open.value) { close(); return; }
  if (!call.active) return;
  trigger.value?.focus({ preventScroll: true });
  inputs.value = null;
  readError.value = false;
  selectionError.value = '';
  open.value = true;
};
watch(open, async (shown, _, onCleanup) => {
  visit++;
  if (!shown) { unregisterModal(panelId); return; }
  registerModal(panelId, close, () => panel.value);
  onCleanup(onClickOutside(panel, close, { ignore: [trigger] }));
  await nextTick();
  if (open.value) panel.value?.focus({ preventScroll: true });
}, { flush: 'sync' });
watch(() => call.active, active => { if (!active) close(); }, { flush: 'sync' });
onBeforeUnmount(close);

// Anchor above the whole control so the menu leaves both the mic and its label exposed.
watch([panel, bounds], ([menu, frame], _, onCleanup) => {
  const control = trigger.value?.parentElement;
  if (!menu || !frame || !control) return;
  const place = () => {
    const anchor = control.getBoundingClientRect();
    const area = frame.getBoundingClientRect();
    const gap = 6;
    const above = Math.max(0, anchor.top - area.top - gap);
    const below = Math.max(0, area.bottom - anchor.bottom - gap);
    const upwards = above >= below;
    menu.style.left = `${Math.max(0, Math.min(anchor.left - area.left, area.width - menu.offsetWidth))}px`;
    menu.style.bottom = upwards ? `${area.height - above}px` : '';
    menu.style.top = upwards ? '' : `${area.height - below}px`;
    menu.style.maxHeight = `${upwards ? above : below}px`;
  };
  place();
  const observer = new ResizeObserver(place);
  observer.observe(frame);
  observer.observe(control);
  window.addEventListener('scroll', place, true);
  window.visualViewport?.addEventListener('scroll', place);
  onCleanup(() => {
    observer.disconnect();
    window.removeEventListener('scroll', place, true);
    window.visualViewport?.removeEventListener('scroll', place);
  });
}, { flush: 'post' });

const moveFocus = (event: KeyboardEvent) => {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const items = Array.from(panel.value?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
  if (!items.length) return;
  event.preventDefault();
  const index = items.indexOf(document.activeElement as HTMLButtonElement);
  let next = 0;
  if (event.key === 'End') next = items.length - 1;
  if (event.key === 'ArrowDown') next = (index + 1) % items.length;
  if (event.key === 'ArrowUp') next = (index < 0 ? items.length - 1 : (index + items.length - 1) % items.length);
  items[next]?.focus();
};

// Route changes also occur while muted. Read only while the picker is visible,
// and wait for each native response before scheduling the next read.
watch([open, visibility, selecting, refresh, () => call.microphoneState], ([shown, visible, switching], _, onCleanup) => {
  if (!shown || visible !== 'visible' || switching) return;
  let live = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => { live = false; clearTimeout(timer); });
  const read = async () => {
    try {
      const current = await call.listInputs();
      if (!live) return;
      inputs.value = current;
      readError.value = false;
      timer = setTimeout(read, 2000);
    } catch {
      if (!live) return;
      inputs.value = null;
      readError.value = true;
    }
  };
  void read();
});

const select = async (id: string | null) => {
  if (!open.value || !inputs.value || selecting.value || call.microphoneState === 'requesting') return;
  const currentVisit = visit;
  selecting.value = true;
  selectionError.value = '';
  try {
    await call.selectInput(id);
    if (visit === currentVisit) close();
  } catch (error) {
    if (visit === currentVisit) {
      console.warn('[VoiceInputPicker]', String(error).slice(0, 512));
      selectionError.value = '切换失败';
    }
  } finally {
    selecting.value = false;
  }
};
const choices = computed(() => [
  { id: null, label: '自动选择' }, ...(inputs.value?.routeOptions ?? []), ...(inputs.value?.devices ?? []),
]);
const unavailablePreference = computed(() => !!inputs.value && call.inputDeviceId !== null
  && !choices.value.some(choice => choice.id === call.inputDeviceId));
</script>

<template>
  <button ref="trigger" type="button" class="voice-input-trigger" aria-label="选择麦克风" aria-haspopup="menu"
    :aria-expanded="open" :aria-controls="open ? panelId : undefined" @click="toggle">
    麦克风 <ChevronDown :size="13" aria-hidden="true" />
  </button>
  <Teleport v-if="open" to="#vcp-feature-overlays">
    <div class="vcp-viewport-surface fixed inset-0 z-sheet pointer-events-none">
      <div ref="bounds" class="voice-input-bounds">
        <section :id="panelId" ref="panel" role="menu" aria-label="选择麦克风" tabindex="-1"
          class="voice-input-panel vcp-scrollable" @keydown="moveFocus" @keydown.esc.stop.prevent="close">
          <template v-if="inputs">
            <button v-for="choice in choices" :key="choice.id === null ? 'auto' : 'device:' + choice.id"
              type="button" role="menuitemradio" :aria-checked="call.inputDeviceId === choice.id"
              :aria-description="choice.id !== null && inputs.activeId === choice.id ? '当前输入' : undefined"
              :disabled="selecting || call.microphoneState === 'requesting'"
              class="voice-input-row" @click="select(choice.id)">
              <Check v-if="call.inputDeviceId === choice.id" class="voice-input-check" :size="16" :stroke-width="2" aria-hidden="true" />
              <span class="voice-input-name">{{ choice.label }}</span>
              <AudioLines v-if="choice.id !== null && inputs.activeId === choice.id" class="voice-input-active"
                :size="15" :stroke-width="1.8" aria-hidden="true" />
            </button>
          </template>
          <p v-if="readError" role="alert" class="voice-input-message">设备列表加载失败</p>
          <button v-if="readError" type="button" role="menuitem" class="voice-input-row" @click="refresh++">
            <span class="voice-input-name">重试</span>
          </button>
          <p v-else-if="!inputs" role="status" class="voice-input-message">加载中…</p>
          <p v-else-if="selecting" role="status" class="voice-input-message">切换中…</p>
          <p v-else-if="selectionError || unavailablePreference" role="alert" class="voice-input-message">{{ selectionError || '所选设备不可用' }}</p>
        </section>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.voice-input-trigger { display: flex; align-items: center; justify-content: center; gap: 2px; min-width: 56px; min-height: 44px; color: inherit; background: transparent; border: 0; font-size: 12px; touch-action: manipulation; }
.voice-input-bounds {
  position: absolute;
  inset: max(8px, var(--vcp-safe-top)) max(8px, var(--vcp-safe-right)) max(8px, var(--vcp-safe-bottom)) max(8px, var(--vcp-safe-left));
}
.voice-input-panel {
  position: absolute;
  width: min(15rem, 100%); overflow-y: auto; overscroll-behavior: contain;
  padding: 4px; border: 1px solid var(--border-color); border-radius: 9px;
  background: var(--secondary-bg); color: var(--primary-text); pointer-events: auto;
  box-shadow: 0 3px 12px rgb(0 0 0 / .13);
}
.voice-input-panel:focus { outline: none; }
.voice-input-message {
  margin: 0; padding: 6px 9px;
  color: var(--secondary-text); font-size: 12px; line-height: 18px;
}
.voice-input-message[role="alert"] { color: var(--danger-color); }
.voice-input-row {
  display: grid; grid-template-columns: 16px minmax(0, 1fr) auto; align-items: center; gap: 6px;
  width: 100%; min-height: 44px; padding: 4px 8px; border-radius: 5px;
  color: inherit; text-align: start; font-size: 14px; line-height: 1.4; touch-action: manipulation;
}
.voice-input-name { grid-column: 2; overflow-wrap: anywhere; }
.voice-input-active { color: var(--secondary-text); }
.voice-input-row:active:not(:disabled), .voice-input-row:focus-visible { background: var(--accent-bg); }
.voice-input-row:disabled { opacity: .55; }
button:focus-visible { outline: 2px solid var(--highlight-text); outline-offset: -2px; }
</style>
