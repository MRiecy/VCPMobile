<script setup lang="ts">
import { computed, ref } from "vue";
import { Zap, Smartphone, Lock, ShieldCheck } from "lucide-vue-next";

interface OemGuide {
  brand: string;
  shortName: string;
  keywords: string[];
  steps: string[];
}

const guides: OemGuide[] = [
  {
    brand: "OPPO / realme / 一加 (ColorOS)",
    shortName: "ColorOS",
    keywords: ["coloros", "oppo", "realme", "oneplus"],
    steps: [
      "设置 → 电池 → 应用耗电管理 → VCP Mobile → 开启「允许完全后台行为」",
      "设置 → 电池 → 应用速冻 → VCP Mobile → 关闭「自动速冻」",
      "设置 → 应用 → 自启动管理 → VCP Mobile → 开启「自启动」与「后台自启动」",
      "设置 → 电池 → 更多设置 → 关闭「睡眠待机优化」",
    ],
  },
  {
    brand: "小米 / Redmi / HyperOS",
    shortName: "Xiaomi",
    keywords: ["miui", "xiaomi", "redmi", "poco", "hyperos"],
    steps: [
      "设置 → 应用设置 → 应用管理 → VCP Mobile → 省电策略 → 选择「无限制」",
      "设置 → 应用设置 → 应用管理 → VCP Mobile → 开启「自启动」",
      "设置 → 应用设置 → 应用管理 → VCP Mobile → 权限管理 → 允许「显示后台弹出界面」",
    ],
  },
  {
    brand: "华为 / 荣耀 (HarmonyOS)",
    shortName: "Huawei",
    keywords: ["harmony", "huawei", "honor", "emui"],
    steps: [
      "设置 → 电池 → 应用启动管理 → VCP Mobile → 关闭「自动管理」",
      "手动开启「允许自启动」「允许关联启动」「允许后台活动」",
      "设置 → 电池 → 更多电池设置 → 开启「休眠时始终保持网络连接」",
    ],
  },
  {
    brand: "vivo / iQOO (OriginOS)",
    shortName: "vivo",
    keywords: ["funtouch", "origin", "vivo", "iqoo"],
    steps: [
      "设置 → 电池 → 后台高耗电 → VCP Mobile → 开启「允许高耗电运行」",
      "设置 → 电池 → 耗电保护 → VCP Mobile → 选择「不限制后台运行」",
      "设置 → 权限管理 → 自启动 → VCP Mobile → 开启「自启动」",
    ],
  },
  {
    brand: "三星 (One UI)",
    shortName: "Samsung",
    keywords: ["samsung", "one ui"],
    steps: [
      "设置 → 电池 → 后台使用限制 → 深度休眠应用程序 → 排除 VCP Mobile",
      "设置 → 电池 → 用电不受限制的应用 → 添加 VCP Mobile",
    ],
  },
];

const detectedBrand = computed(() => {
  const ua = navigator.userAgent.toLowerCase();
  for (const guide of guides) {
    if (guide.keywords.some((k) => ua.includes(k))) {
      return guide.brand;
    }
  }
  return null;
});

// 如果检测不到，默认选中第一个
const selectedBrand = ref<string>(detectedBrand.value || guides[0].brand);

const activeGuide = computed(() => {
  return guides.find((g) => g.brand === selectedBrand.value) || guides[0];
});

// 富文本渲染：高亮箭头和操作项
const highlightStep = (text: string) => {
  return text
    .replace(/→/g, '<span class="opacity-30 mx-1.5 text-[9px]">▶</span>')
    .replace(
      /「(.*?)」/g,
      '<span class="text-blue-600 dark:text-blue-400 font-bold bg-blue-500/10 px-1.5 py-0.5 rounded text-[11px] mx-0.5 border border-blue-500/20">$1</span>'
    );
};
</script>

<template>
  <div class="space-y-6">
    <!-- 顶部高亮预警卡片 -->
    <div class="flex gap-3 p-3.5 rounded-xl bg-amber-500/10 border border-amber-500/20 text-amber-600 dark:text-amber-400">
      <Zap class="w-5 h-5 shrink-0 mt-0.5" />
      <div class="space-y-1">
        <h3 class="text-xs font-bold uppercase tracking-wider">系统级保活配置</h3>
        <p class="text-[11px] leading-relaxed opacity-80">
          国内定制系统会激进地冻结后台进程。为保证 Agent 响应与离线消息同步不被断开，请务必按以下步骤配置您的设备。
        </p>
      </div>
    </div>

    <!-- 品牌选项卡 (Segmented Control) -->
    <div class="flex overflow-x-auto hide-scrollbar gap-1 p-1 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-lg">
      <button
        v-for="guide in guides"
        :key="guide.brand"
        @click="selectedBrand = guide.brand"
        class="flex-1 min-w-[70px] px-3 py-1.5 rounded-md text-[11px] font-medium transition-all whitespace-nowrap flex items-center justify-center gap-1.5"
        :class="selectedBrand === guide.brand ? 'bg-[var(--bg-color)] shadow-sm text-[var(--primary-text)]' : 'text-primary-text/50 hover:text-primary-text/80'"
      >
        <Smartphone v-if="detectedBrand === guide.brand" class="w-3 h-3 text-blue-500" />
        {{ guide.shortName }}
      </button>
    </div>

    <!-- 垂直时间线步骤引导 -->
    <div class="px-2 pb-2">
      <h4 class="text-xs font-bold mb-4 opacity-80 flex items-center gap-2">
        <span>{{ activeGuide.brand }}</span>
        <span v-if="detectedBrand === activeGuide.brand" class="px-1.5 py-0.5 rounded text-[9px] bg-blue-500/10 text-blue-500 border border-blue-500/20">当前设备</span>
      </h4>
      <div class="relative pl-6">
        <!-- 左侧连接线 -->
        <div class="absolute left-[11px] top-2 bottom-2 w-px bg-[var(--border-color)]"></div>

        <div v-for="(step, i) in activeGuide.steps" :key="i" class="relative mb-6 last:mb-0">
          <!-- 步骤节点 -->
          <div class="absolute -left-[27px] top-0 w-6 h-6 rounded-full bg-[var(--bg-color)] border border-[var(--border-color)] flex items-center justify-center text-[10px] font-mono font-bold text-primary-text/60 z-10">
            {{ i + 1 }}
          </div>
          <!-- 步骤内容 -->
          <div class="text-[12px] leading-relaxed text-primary-text/80 pt-0.5" v-html="highlightStep(step)"></div>
        </div>
      </div>
    </div>

    <!-- 终极防御：物理锁定 -->
    <div class="pt-5 border-t border-[var(--border-color)] border-dashed space-y-3 px-1">
      <div class="flex items-center gap-2 text-primary-text/50">
        <ShieldCheck class="w-4 h-4" />
        <h4 class="text-[11px] font-black uppercase tracking-widest">终极防御 / Ultimate Defense</h4>
      </div>
      
      <div class="flex items-start gap-3 p-3 rounded-lg bg-[var(--secondary-bg)]/50 border border-[var(--border-color)]">
        <div class="w-8 h-8 rounded-full bg-white/5 border border-white/10 flex items-center justify-center shrink-0">
          <Lock class="w-4 h-4 text-primary-text/70" />
        </div>
        <div class="space-y-1">
          <h5 class="text-xs font-bold">物理锁定多任务卡片</h5>
          <p class="text-[11px] text-primary-text/60 leading-relaxed">
            如按上述配置后仍被杀后台，请呼出手机<strong class="text-primary-text/90">多任务界面</strong>（从屏幕底部上滑并停顿），找到 VCP Mobile 卡片并<strong class="text-primary-text/90">向下拉动</strong>，使其出现 🔒 锁形图标。
          </p>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.hide-scrollbar {
  -ms-overflow-style: none;
  scrollbar-width: none;
}
.hide-scrollbar::-webkit-scrollbar {
  display: none;
}
</style>
