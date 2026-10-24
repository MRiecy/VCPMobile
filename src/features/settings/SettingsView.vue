<script setup lang="ts">
import { ref, watch, computed, defineComponent, onErrorCaptured, onScopeDispose } from "vue";
import { useThemeStore } from "../../core/stores/theme";
import { LAYER_PAGE_BASE } from "../../core/constants/layers";
import SlidePage from "../../components/ui/SlidePage.vue";

import SettingsRow from "../../components/settings/SettingsRow.vue";
import FluidBackground from "../../components/ui/WebGLFluidBackground.vue";
import { settingsPages } from "./catalog";
import { useSettingsNavigation } from "./useSettingsNavigation";
import { useDeveloperSettingsStore } from './developer/store';
import { useSettingsStore } from '../../core/stores/settings';

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: LAYER_PAGE_BASE,
  },
);

const emit = defineEmits<{
  close: [];
}>();

const subpageSurface = ref<HTMLElement | null>(null);
const {
  activeVisit, displayedVisit, pageError, navigationError, attempt, loading, loaded,
  openPage, openDeveloperPage, requestLeave, failPage, retryPage, finishPageLeave,
} = useSettingsNavigation({
  isOpen: computed(() => props.isOpen),
  surface: () => subpageSurface.value,
  close: () => emit("close"),
});

const developerSettings = useDeveloperSettingsStore();
const rootPages = settingsPages.filter((page) => !page.parent);
const openDeveloper = () => { if (developerSettings.enabled) openDeveloperPage(); };

const displayedPage = computed(() => settingsPages.find((page) => page.id === displayedVisit.value?.id));
const subPageTitle = computed(() => settingsPages.find((page) => page.id === activeVisit.value?.id)?.title || "");
const pageActive = computed(() => props.isOpen && activeVisit.value === displayedVisit.value && !!activeVisit.value);
const settingsStore = useSettingsStore();
const homeActive = computed(() => props.isOpen && !displayedVisit.value);
const reloadSettings = async () => {
  if (!homeActive.value || settingsStore.loading) return;
  try { await settingsStore.fetchSettings(); } catch { /* The Store owns the displayed read error. */ }
};
// Outer close retains the current picture; its own after-leave releases the child.
const pageVisible = computed(() => !!activeVisit.value || !props.isOpen);

const PageLoadBoundary = defineComponent({
  props: { visit: { type: Number, required: true } },
  emits: { error: (_visit: number, _error: unknown) => true },
  setup(boundaryProps, { slots, emit: emitError }) {
    onErrorCaptured((error) => {
      emitError('error', boundaryProps.visit, error);
      return false;
    });
    return () => slots.default?.();
  },
});

const finishSubpageTransition = (element: Element) => {
  finishPageLeave(Number((element as HTMLElement).dataset.settingsVisit));
};
const finishSettingsTransition = (element: Element) => {
  finishPageLeave(Number((element as HTMLElement).dataset.settingsClosingVisit));
};
const rememberClosingVisit = (element: Element) => {
  (element as HTMLElement).dataset.settingsClosingVisit = String(displayedVisit.value?.serial ?? '');
};

// 子页面预热：设置页打开后，在浏览器空闲时后台预载主题/关于子页的异步 chunk。
// 否则进入子页时「chunk 下载 → fetchThemes → 预览区空态切内容 → 卡片轨吸附定位」
// 全部挤在 0.35s 滑入动画中段，产生果冻式顿挫（加载越慢越明显）。
// 预热的成果是幂等缓存：ThemePicker 挂载时若清单已在内存则不再重复拉取。
// 注意：清单本身由 themeStore.initTheme 在启动 idle 时预扫描（eager glob，纯内存组装），
// 这里真正需要预热的只有 chunk；挂到 idle 是为避免与设置页首开动画
// 争抢主线程——与 initTheme 的空闲加载模式保持同构，不开同步预热的后门。
const themeStore = useThemeStore();
let idleWarmup: number | null = null;
let warmupTimeout: ReturnType<typeof setTimeout> | null = null;
const warmSubPages = () => {
  if (idleWarmup !== null || warmupTimeout !== null) return;
  const preload = () => {
    idleWarmup = null;
    warmupTimeout = null;
    if (themeStore.availableThemes.length === 0) void themeStore.fetchThemes();
    // A failed optional preload must leave the actual page loader free to retry.
    void import("./ThemePicker.vue").catch(() => {});
    void import("./components/AboutSection.vue").catch(() => {});
  };
  if (window.requestIdleCallback) idleWarmup = window.requestIdleCallback(preload);
  else warmupTimeout = setTimeout(preload, 1000);
};

watch(() => props.isOpen, (open) => { if (open) warmSubPages(); }, { immediate: true });
onScopeDispose(() => {
  if (idleWarmup !== null) window.cancelIdleCallback?.(idleWarmup);
  if (warmupTimeout !== null) clearTimeout(warmupTimeout);
});

// ── 关于页：独立沉浸式覆盖层 ─────────────────────────────────────────────
// 关于页是全局设置中唯一的全屏沉浸页（自带返回按钮与 WebGL 流体背景），
// 因此不套用列表子页的侧滑模板，而是作为覆盖整个设置页（含头部）的独立层，
// 以淡入+轻微上浮进场。背景宿主常驻保活（v-show），经 idle 预编译着色器，
// 仅在关于页可见期间运行渲染循环。
const fluidBgRef = ref<{ setActive: (value: boolean) => void } | null>(null);
const aboutDisplayed = computed(() => displayedPage.value?.frame === "immersive");
const aboutActive = computed(() => aboutDisplayed.value && pageActive.value);
watch([aboutActive, fluidBgRef], ([active]) => fluidBgRef.value?.setActive(active), { flush: 'sync' });
onScopeDispose(() => fluidBgRef.value?.setActive(false));
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex"
    @before-leave="rememberClosingVisit" @after-leave="finishSettingsTransition">
    <div
      ref="subpageSurface"
      class="settings-view relative flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto"
    >
      <!-- Header -->
      <header
        :inert="aboutDisplayed || !props.isOpen"
        class="border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0"
      >
        <div class="flex items-center justify-between w-full min-w-0"
          :class="activeVisit ? 'px-4' : 'mx-auto max-w-3xl px-4 sm:px-6'">
          <h2 class="text-xl font-bold">{{ activeVisit ? subPageTitle : '全局设置' }}</h2>
          <button
            type="button"
            :aria-label="activeVisit?.id === 'developer' ? '返回关于' : activeVisit ? '返回设置首页' : '关闭设置'"
            @click="requestLeave"
            class="min-h-11 min-w-11 p-2 transition-opacity flex items-center justify-center opacity-70 active:opacity-100"
          >
            <svg
              v-if="activeVisit"
              width="22"
              height="22"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="m15 18-6-6 6-6"/>
            </svg>
            <svg
              v-else
              width="22"
              height="22"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>
      </header>

      <!-- Scrollable Area -->
      <div class="flex-1 min-h-0 relative overflow-hidden">
        <!-- 主页 -->
        <div :inert="!!displayedVisit || !props.isOpen"
          class="h-full overflow-y-auto no-rubber-band">
          <div class="mx-auto flex min-h-full w-full max-w-3xl min-w-0 flex-col px-3 pt-3 pb-[calc(16px+var(--vcp-safe-bottom,0px))] sm:px-5">
            <p v-if="settingsStore.error" role="alert" class="shrink-0 px-1 text-sm text-red-500 break-words">
              {{ settingsStore.error }}
              <button type="button" class="ml-2 underline" :disabled="!homeActive || settingsStore.loading" @click="reloadSettings">重新加载</button>
            </p>
            <div class="shrink-0 divide-y divide-black/5 dark:divide-white/5">
              <SettingsRow
                v-for="cat in rootPages"
                :key="cat.id"
                :title="cat.title"
                :description="cat.description"
                clickable
                @click="openPage(cat.id)"
              />
            </div>

            <!-- Footer -->
            <div class="mt-auto shrink-0 px-1 pt-6 text-center break-words opacity-15 text-[8px] font-mono uppercase tracking-[0.25em] select-none">
              VCP MOBILE // PROJECT AVATAR // 2026.04.07
            </div>
          </div>
        </div>

        <!-- 子页面层 -->
        <Transition name="slide-subpage" @after-leave="finishSubpageTransition">
          <div
            v-if="displayedVisit && displayedPage && displayedPage.frame !== 'immersive'"
            v-show="pageVisible"
            :key="displayedVisit.serial"
            :data-settings-visit="displayedVisit.serial"
            :inert="!pageActive"
            class="absolute inset-0 flex flex-col z-10 transition-colors duration-300 bg-[var(--primary-bg)]"
          >
            <p v-if="navigationError" class="px-3 py-2 text-sm text-red-500" role="alert">{{ navigationError }}</p>
            <div
              class="flex-1 min-h-0"
              :class="displayedPage.frame === 'viewport'
                ? 'overflow-hidden flex flex-col'
                : 'overflow-y-auto px-3 pt-3 no-rubber-band pb-[calc(var(--vcp-safe-bottom,0px)+32px)]'"
            >
              <div v-if="pageError" class="p-3 space-y-3">
                <p role="alert" class="text-sm text-red-500">页面加载失败：{{ pageError }}</p>
                <button type="button" class="min-h-11 px-3 border rounded-lg" @click="retryPage">重试</button>
              </div>
              <template v-else>
                <p v-if="loading" class="p-3 text-sm opacity-60" role="status">正在加载设置…</p>
                <PageLoadBoundary :key="`${displayedVisit.serial}:${attempt}`" :visit="displayedVisit.serial" @error="failPage">
                  <component v-if="pageActive || loaded" :is="displayedPage.component" :ref="displayedVisit.setHandle"
                    :active="pageActive" @back="requestLeave" />
                </PageLoadBoundary>
              </template>
            </div>
          </div>
        </Transition>
      </div>

      <!-- 关于页流体背景宿主：常驻保活（v-show），idle 预编译着色器，仅关于页可见时渲染 -->
      <FluidBackground
        ref="fluidBgRef"
        v-show="aboutDisplayed"
        class="about-fluid z-20"
        :class="{ 'about-fluid-active': aboutDisplayed && pageVisible }"
      />

      <!-- 关于页：独立全屏沉浸层，覆盖整个设置页（含头部），不套用子页面侧滑模板 -->
      <Transition name="about-immersive" @after-leave="finishSubpageTransition">
        <div v-if="displayedVisit && displayedPage && aboutDisplayed" v-show="pageVisible"
          :key="displayedVisit.serial" :data-settings-visit="displayedVisit.serial"
          :inert="!pageActive" class="absolute inset-0 z-30">
          <div v-if="pageError" class="p-4 pt-[calc(var(--vcp-safe-top,24px)+16px)] space-y-3">
            <p role="alert" class="text-sm text-red-500">页面加载失败：{{ pageError }}</p>
            <button type="button" class="min-h-11 px-3 border rounded-lg" @click="retryPage">重试</button>
            <button type="button" class="min-h-11 px-3" @click="requestLeave">返回设置</button>
          </div>
          <template v-else>
            <div v-if="loading" class="p-4 pt-[calc(var(--vcp-safe-top,24px)+16px)]">
              <p role="status" class="text-sm opacity-60">正在加载设置…</p>
              <button type="button" class="min-h-11" @click="requestLeave">返回设置</button>
            </div>
            <PageLoadBoundary :key="`${displayedVisit.serial}:${attempt}`" :visit="displayedVisit.serial" @error="failPage">
              <component v-if="pageActive || loaded" :is="displayedPage.component" :ref="displayedVisit.setHandle"
                :active="pageActive" @back="requestLeave" @open-developer="openDeveloper" />
            </PageLoadBoundary>
          </template>
        </div>
      </Transition>
    </div>
  </SlidePage>
</template>

<style scoped>
.settings-view {
  background-color: var(--primary-bg);
}

.slide-subpage-enter-active {
  transition: transform 0.35s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-subpage-leave-active {
  transition: transform 0.3s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-subpage-enter-from,
.slide-subpage-leave-to {
  transform: translateX(100%);
}

/* 关于页背景宿主常驻，退出期间保留画面而停止循环。 */
.about-fluid {
  opacity: 0;
  transition: opacity 0.35s ease;
}

.about-fluid-active {
  opacity: 1;
  transition-duration: 0.5s;
}

/* 关于页独立进场：淡入 + 轻微上浮，与列表子页的右滑形成明确的层级区分 */
.about-immersive-enter-active {
  transition:
    opacity 0.4s ease,
    transform 0.45s cubic-bezier(0.22, 1, 0.36, 1);
}

.about-immersive-leave-active {
  transition:
    opacity 0.28s ease,
    transform 0.32s cubic-bezier(0.32, 0.72, 0, 1);
}

.about-immersive-enter-from {
  opacity: 0;
  transform: translateY(2.5%) scale(0.985);
}

.about-immersive-leave-to {
  opacity: 0;
  transform: translateY(1.5%);
}
</style>
