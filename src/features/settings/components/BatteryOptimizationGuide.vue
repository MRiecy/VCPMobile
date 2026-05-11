<script setup lang="ts">
import { computed, ref } from "vue";
import { Zap, Smartphone } from "lucide-vue-next";
import SettingsCard from "../../../components/settings/SettingsCard.vue";

interface OemGuide {
  brand: string;
  keywords: string[];
  steps: string[];
  intent?: string;
}

const guides: OemGuide[] = [
  {
    brand: "小米 / Redmi / POCO",
    keywords: ["miui", "xiaomi", "redmi", "poco"],
    steps: [
      "设置 → 应用设置 → 应用管理",
      "找到 VCP Mobile → 省电策略",
      "选择「无限制」",
      "返回 → 权限管理 → 允许「自启动」和「后台弹出界面」",
    ],
  },
  {
    brand: "华为 / 荣耀",
    keywords: ["harmony", "huawei", "honor", "emui"],
    steps: [
      "设置 → 应用和服务 → 应用启动管理",
      "找到 VCP Mobile",
      "关闭「自动管理」",
      "手动开启「允许自启动」「允许关联启动」「允许后台活动」",
    ],
  },
  {
    brand: "OPPO / realme / 一加",
    keywords: ["coloros", "oppo", "realme", "oneplus"],
    steps: [
      "设置 → 电池 → 应用耗电管理",
      "找到 VCP Mobile",
      "开启「允许完全后台行为」",
      "开启「允许应用自启动」",
    ],
  },
  {
    brand: "vivo / iQOO",
    keywords: ["funtouch", "origin", "vivo", "iqoo"],
    steps: [
      "设置 → 电池 → 后台耗电管理",
      "找到 VCP Mobile",
      "选择「允许后台高耗电」",
    ],
  },
  {
    brand: "三星",
    keywords: ["samsung", "one ui"],
    steps: [
      "设置 → 电池 → 后台使用限制",
      "深度休眠应用程序 → 排除 VCP Mobile",
      "或者：设置 → 电池 → 用电不受限制的应用 → 添加 VCP Mobile",
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

const selectedBrand = ref<string | null>(detectedBrand.value);

const filteredGuides = computed(() => {
  if (!selectedBrand.value) return guides;
  const g = guides.find((g) => g.brand === selectedBrand.value);
  return g ? [g] : guides;
});

const showAll = () => {
  selectedBrand.value = null;
};

// const openBatterySettings = () => {
//   // Android 原生电池优化设置 Intent（通过 opener 插件打开）
//   // 这里仅作为占位，实际需要通过 Rust/bridge 触发系统 Intent
//   console.log("[BatteryGuide] Request open battery settings");
// };
</script>

<template>
  <div class="space-y-5">
    <!-- 顶部提示 -->
    <div class="bg-yellow-500/10 border border-yellow-500/20 rounded-xl p-4 space-y-2">
      <div class="flex items-center gap-2 text-yellow-400">
        <Zap class="w-4 h-4" />
        <span class="text-xs font-bold uppercase tracking-wider">后台保活建议</span>
      </div>
      <p class="text-xs text-primary-text/70 leading-relaxed">
        Android 系统为省电会主动冻结后台应用。即使前台服务已启用，仍建议将 VCP Mobile
        加入电池白名单，确保 Agent 流式响应和消息同步不被中断。
      </p>
    </div>

    <!-- 设备检测 -->
    <div v-if="detectedBrand" class="flex items-center gap-3 px-1">
      <Smartphone class="w-4 h-4 text-blue-400" />
      <span class="text-xs text-primary-text/60">
        检测到设备：<strong class="text-primary-text">{{ detectedBrand }}</strong>
      </span>
      <button
        v-if="selectedBrand"
        @click="showAll"
        class="text-xs text-blue-400 hover:text-blue-300 transition-colors ml-auto"
      >
        查看全部
      </button>
    </div>

    <!-- 品牌选择（未检测到时显示） -->
    <div v-else class="flex flex-wrap gap-2">
      <button
        v-for="guide in guides"
        :key="guide.brand"
        @click="selectedBrand = guide.brand"
        class="text-[10px] px-2.5 py-1.5 rounded-lg bg-white/5 border border-white/10 hover:bg-white/10 transition-colors"
        :class="selectedBrand === guide.brand ? 'bg-blue-500/20 border-blue-500/30 text-blue-300' : 'text-primary-text/60'"
      >
        {{ guide.brand }}
      </button>
    </div>

    <!-- 引导卡片 -->
    <div class="space-y-4">
      <SettingsCard
        v-for="guide in filteredGuides"
        :key="guide.brand"
        no-padding
      >
        <div class="p-4 space-y-3">
          <h4 class="text-sm font-bold">{{ guide.brand }}</h4>
          <ol class="space-y-2">
            <li
              v-for="(step, i) in guide.steps"
              :key="i"
              class="flex items-start gap-2.5 text-xs text-primary-text/70"
            >
              <span
                class="shrink-0 w-5 h-5 rounded-full bg-white/5 flex items-center justify-center text-[10px] font-mono font-bold mt-0.5"
              >
                {{ i + 1 }}
              </span>
              <span class="leading-relaxed">{{ step }}</span>
            </li>
          </ol>
        </div>
      </SettingsCard>
    </div>

    <!-- 通用提示 -->
    <div class="space-y-3">
      <h4 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 px-1">
        通用操作
      </h4>
      <SettingsCard no-padding>
        <div class="p-4 space-y-3">
          <div class="flex items-center justify-between">
            <div class="space-y-1">
              <p class="text-sm font-medium">最近任务锁定</p>
              <p class="text-[11px] text-primary-text/50">
                多任务界面长按 VCP Mobile 卡片 → 点击「锁定」图标
              </p>
            </div>
          </div>
          <div class="h-px bg-white/5" />
          <div class="flex items-center justify-between">
            <div class="space-y-1">
              <p class="text-sm font-medium">系统电池优化豁免</p>
              <p class="text-[11px] text-primary-text/50">
                设置 → 应用 → 特殊应用权限 → 电池优化 → VCP Mobile → 不优化
              </p>
            </div>
          </div>
        </div>
      </SettingsCard>
    </div>
  </div>
</template>
