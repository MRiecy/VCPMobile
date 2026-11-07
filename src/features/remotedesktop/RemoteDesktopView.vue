<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue';
import { ChevronLeft, RefreshCw, Settings2, Keyboard, Send, MonitorPlay } from "@lucide/vue";
import SlidePage from '../../components/ui/SlidePage.vue';
import { useRemoteDesktopStore } from './remoteDesktopStore';
import { BRIDGE_CONFIG_SOURCE_LABELS } from '../../core/bridge/bridgeConfig';

const props = defineProps<{
  isOpen: boolean;
  zIndex: number;
}>();

const emit = defineEmits<{ close: [] }>();

const store = useRemoteDesktopStore();
const status = ref('准备中');
const keyboardOpen = ref(false);
const text = ref('');
const showSettings = ref(false);
const baseUrlDraft = ref(store.baseUrl);
const tokenDraft = ref(store.token);

const bridgeSourceLabel = computed(
  () => BRIDGE_CONFIG_SOURCE_LABELS[store.configSource] || '未知来源',
);

/** 画质 / 分辨率预设（即时生效，下一次抓帧使用）。 */
const qualityPresets = [
  { label: '流畅', value: 30 },
  { label: '均衡', value: 45 },
  { label: '清晰', value: 62 },
  { label: '高清', value: 80 },
];
const widthPresets = [
  { label: '原图', value: 0 },
  { label: '1280', value: 1280 },
  { label: '960', value: 960 },
  { label: '720', value: 720 },
  { label: '540', value: 540 },
];

function setDisplayQuality(value: number): void {
  store.updateDisplaySettings({ quality: value });
}

function setDisplayMaxWidth(value: number): void {
  store.updateDisplaySettings({ maxWidth: value });
}

let loopTimer: ReturnType<typeof setTimeout> | null = null;
let polling = false;

const activeTitle = computed(() => store.activePage?.title || '桌面 VCPChat');

async function tick(): Promise<void> {
  if (!props.isOpen) return;
  if (!polling) {
    polling = true;
    try {
      await store.capture();
      status.value = `${store.frameWidth}×${store.frameHeight} · ${Math.round(store.frameBytes / 1024)}KB · ${store.latencyMs}ms`;
      store.clearError();
    } catch (error) {
      store.setError(String((error as Error)?.message || error));
      status.value = '抓帧失败';
    } finally {
      polling = false;
    }
  }
  loopTimer = setTimeout(tick, 650);
}

async function bootstrap(): Promise<void> {
  try {
    await store.refreshPages();
    const first = store.pages.find((page) => page.url.endsWith('main.html')) || store.pages[0];
    if (first) store.activePageId = first.id;
    status.value = `已连接 · ${store.pages.length} 个窗口`;
    await store.capture();
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
    status.value = '连接失败';
  }
  tick();
}

function stopLoop(): void {
  if (loopTimer) clearTimeout(loopTimer);
  loopTimer = null;
}

watch(
  () => props.isOpen,
  (open) => {
    if (open) {
      baseUrlDraft.value = store.baseUrl;
      tokenDraft.value = store.token;
      bootstrap();
    } else {
      stopLoop();
    }
  },
  { immediate: true },
);

onUnmounted(stopLoop);

async function onFrameClick(event: MouseEvent): Promise<void> {
  const target = event.currentTarget as HTMLElement;
  const rect = target.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const nx = (event.clientX - rect.left) / rect.width;
  const ny = (event.clientY - rect.top) / rect.height;
  try {
    await store.tap(nx, ny);
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
  }
}

let touchStartY = 0;
let touchMoved = false;

function onTouchStart(event: TouchEvent): void {
  const touch = event.touches[0];
  touchStartY = touch.clientY;
  touchMoved = false;
}

function onTouchMove(event: TouchEvent): void {
  if (Math.abs(event.touches[0].clientY - touchStartY) > 8) touchMoved = true;
}

async function onTouchEnd(event: TouchEvent): Promise<void> {
  const touch = event.changedTouches[0];
  if (touchMoved) {
    const deltaY = touchStartY - touch.clientY;
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    try {
      await store.scroll(
        (touch.clientX - rect.left) / rect.width,
        (touch.clientY - rect.top) / rect.height,
        Math.round(deltaY * 2),
      );
    } catch (error) {
      store.setError(String((error as Error)?.message || error));
    }
  }
}

async function onPageChange(event: Event): Promise<void> {
  store.activePageId = (event.target as HTMLSelectElement).value;
  try {
    await store.capture();
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
  }
}

async function sendText(): Promise<void> {
  const value = text.value;
  if (!value) return;
  try {
    await store.typeText(value);
    text.value = '';
    if (keyboardOpen.value) {
      await store.sendKey('Enter');
    }
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
  }
}

async function sendKey(key: string): Promise<void> {
  try {
    await store.sendKey(key);
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
  }
}

async function applySettings(): Promise<void> {
  store.updateSettings({ baseUrl: baseUrlDraft.value, token: tokenDraft.value });
  showSettings.value = false;
  await bootstrap();
}

async function reload(): Promise<void> {
  try {
    await store.refreshPages();
    await store.capture();
  } catch (error) {
    store.setError(String((error as Error)?.message || error));
  }
}
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <main class="h-full min-h-0 flex flex-col bg-black text-white no-swipe">
      <header class="shrink-0 px-2 flex items-center gap-2 bg-[var(--primary-bg)] text-[var(--primary-text)] border-b border-white/10" style="padding-top: var(--vcp-safe-top, 24px); min-height: calc(3rem + var(--vcp-safe-top, 24px));">
        <button type="button" class="w-9 h-9 flex items-center justify-center rounded-full active:scale-95" aria-label="返回" @click="emit('close')">
          <ChevronLeft :size="20" />
        </button>
        <div class="flex-1 min-w-0">
          <div class="text-[9px] font-black tracking-[0.18em] opacity-45">REMOTE DESKTOP</div>
          <div class="text-[12px] font-bold truncate">{{ activeTitle }}</div>
        </div>
        <select
          class="max-w-[120px] h-8 rounded-md bg-[var(--secondary-bg)] text-[10px] px-1 outline-none"
          :value="store.activePageId"
          @change="onPageChange"
        >
          <option v-for="page in store.pages" :key="page.id" :value="page.id">{{ page.title }}</option>
        </select>
        <button type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="刷新" @click="reload">
          <RefreshCw :size="16" />
        </button>
        <button type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="设置" @click="showSettings = !showSettings">
          <Settings2 :size="16" />
        </button>
      </header>

      <div class="relative flex-1 min-h-0 overflow-hidden bg-black">
        <img
          v-if="store.frame"
          :src="store.frame"
          class="absolute inset-0 w-full h-full object-contain select-none"
          draggable="false"
          alt="桌面画面"
          @click="onFrameClick"
          @touchstart.passive="onTouchStart"
          @touchmove.passive="onTouchMove"
          @touchend="onTouchEnd"
        />
        <div v-else class="absolute inset-0 grid place-items-center text-[12px] opacity-70">
          <div class="flex flex-col items-center gap-2">
            <MonitorPlay :size="28" />
            <span>{{ store.error || '正在连接桌面…' }}</span>
          </div>
        </div>

        <div v-if="store.error" class="absolute left-2 right-2 bottom-2 rounded-md bg-red-500/85 text-[10px] px-2 py-1.5">
          {{ store.error }}
        </div>
      </div>

      <footer class="shrink-0 bg-[var(--primary-bg)] text-[var(--primary-text)] border-t border-white/10" style="padding-bottom: var(--vcp-safe-bottom, 0px);">
        <div class="flex items-center gap-2 px-2 py-1.5">
          <button type="button" class="h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[10px] font-bold" @click="keyboardOpen = !keyboardOpen">
            <Keyboard :size="15" />
          </button>
          <input
            v-if="keyboardOpen"
            v-model="text"
            class="flex-1 min-w-0 h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[12px] outline-none"
            placeholder="输入文字后发送到桌面"
            @keydown.enter.prevent="sendText"
          />
          <div v-else class="flex-1 text-[9px] font-mono opacity-55">{{ status }}</div>
          <button v-if="keyboardOpen" type="button" class="h-8 px-2 rounded-md text-[10px] font-bold" style="background-color: var(--highlight-text); color: #fff" @click="sendText">
            <Send :size="14" />
          </button>
          <div class="flex gap-1">
            <button type="button" class="h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[10px]" @click="sendKey('Backspace')">⌫</button>
            <button type="button" class="h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[10px]" @click="sendKey('Enter')">↵</button>
          </div>
        </div>
      </footer>

      <div v-if="showSettings" class="absolute inset-0 z-30 bg-black/60" @click.self="showSettings = false">
        <div class="absolute left-4 right-4 top-16 max-h-[82vh] overflow-y-auto rounded-xl bg-[var(--primary-bg)] text-[var(--primary-text)] p-3 space-y-2">
          <div class="text-[11px] font-bold">桥接设置</div>
          <label class="block text-[9px] opacity-60">Base URL</label>
          <input v-model="baseUrlDraft" class="w-full h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[11px] outline-none" />
          <label class="block text-[9px] opacity-60">Token</label>
          <input v-model="tokenDraft" class="w-full h-8 px-2 rounded-md bg-[var(--secondary-bg)] text-[11px] outline-none" />
          <p class="text-[9px] leading-relaxed opacity-50">
            当前来源：{{ bridgeSourceLabel }}。与「V工程 / 文坊」共用同一份桥接配置，只填 http://桌面IP:5974 会自动补全 /api/mobile-bridge。
          </p>

          <div class="border-t border-black/10 dark:border-white/10 pt-2 space-y-2">
            <div class="flex items-center justify-between">
              <span class="text-[11px] font-bold">画面显示（即时生效）</span>
              <span class="text-[9px] font-mono opacity-55">{{ store.frameWidth }}×{{ store.frameHeight }} · {{ Math.round(store.frameBytes / 1024) }}KB · {{ store.latencyMs }}ms</span>
            </div>
            <div>
              <p class="text-[9px] opacity-60 mb-1">画质（JPEG，越低越省流量）</p>
              <div class="flex gap-1.5">
                <button
                  v-for="preset in qualityPresets"
                  :key="preset.value"
                  type="button"
                  class="flex-1 h-8 rounded-md text-[10px] font-bold border"
                  :class="store.displaySettings.quality === preset.value ? 'border-transparent' : 'border-black/10 dark:border-white/15 opacity-70'"
                  :style="store.displaySettings.quality === preset.value ? 'background-color: var(--highlight-text); color: #fff' : 'background-color: var(--secondary-bg)'"
                  @click="setDisplayQuality(preset.value)"
                >
                  {{ preset.label }} {{ preset.value }}
                </button>
              </div>
            </div>
            <div>
              <p class="text-[9px] opacity-60 mb-1">分辨率（最长边像素，0 = 原图）</p>
              <div class="flex gap-1.5">
                <button
                  v-for="preset in widthPresets"
                  :key="preset.value"
                  type="button"
                  class="flex-1 h-8 rounded-md text-[10px] font-bold border"
                  :class="store.displaySettings.maxWidth === preset.value ? 'border-transparent' : 'border-black/10 dark:border-white/15 opacity-70'"
                  :style="store.displaySettings.maxWidth === preset.value ? 'background-color: var(--highlight-text); color: #fff' : 'background-color: var(--secondary-bg)'"
                  @click="setDisplayMaxWidth(preset.value)"
                >
                  {{ preset.label }}
                </button>
              </div>
            </div>
            <p class="text-[9px] leading-relaxed opacity-50">
              Tailscale 中继较慢时建议「均衡 / 960 或 720」：单帧约 20–45KB，刷新明显更快；桌面端画面原尺寸为 1300×800。
            </p>
          </div>

          <div class="flex justify-end gap-2 pt-1">
            <button type="button" class="h-8 px-3 rounded-md text-[10px]" @click="showSettings = false">取消</button>
            <button type="button" class="h-8 px-3 rounded-md text-[10px] font-bold" style="background-color: var(--highlight-text); color: #fff" @click="applySettings">保存并重连</button>
          </div>
        </div>
      </div>
    </main>
  </SlidePage>
</template>
