<script setup lang="ts">
import { ref, onMounted, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import ModelSelector from '../components/ModelSelector.vue';

interface AgentConfig {
  id: string;
  name: string;
  avatar?: string;
  // Style settings
  disableCustomColors: boolean;
  useThemeColorsInChat: boolean;
  avatarBorderColor: string;
  nameTextColor: string;
  customCss: string;
  cardCss: string;
  chatCss: string;
  // Prompt settings
  systemPrompt: string;
  // Model settings
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  top_p?: number;
  top_k?: number;
  streamOutput: boolean;
  // TTS settings
  ttsVoicePrimary: string;
  ttsRegexPrimary: string;
  ttsVoiceSecondary: string;
  ttsRegexSecondary: string;
  ttsSpeed: number;
}

const props = defineProps<{
  id?: string;
}>();

const emit = defineEmits(['close', 'delete']);

const agentConfig = ref<AgentConfig>({
  id: props.id || '',
  name: '',
  avatar: '',
  disableCustomColors: false,
  useThemeColorsInChat: false,
  avatarBorderColor: '#3d5a80',
  nameTextColor: '#ffffff',
  customCss: '',
  cardCss: '',
  chatCss: '',
  systemPrompt: '',
  model: 'gemini-3-flash-preview',
  temperature: 1.0,
  contextTokenLimit: 1000000,
  maxOutputTokens: 32000,
  top_p: undefined,
  top_k: undefined,
  streamOutput: true,
  ttsVoicePrimary: '',
  ttsRegexPrimary: '',
  ttsVoiceSecondary: '',
  ttsRegexSecondary: '',
  ttsSpeed: 1.0,
});

// UI State
const sections = ref({
  style: false,
  params: false,
  tts: false
});

const toggleSection = (section: keyof typeof sections.value) => {
  sections.value[section] = !sections.value[section];
};

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  agentConfig.value.model = modelId;
};

const isSaving = ref(false);
const saveSuccess = ref(false);
let saveTimeout: ReturnType<typeof setTimeout> | null = null;
let isFirstLoad = true;

const autoSave = async () => {
  if (!agentConfig.value.id) return;
  
  isSaving.value = true;
  saveSuccess.value = false;
  
  try {
    await invoke('save_agent_config', { agent: agentConfig.value });
    saveSuccess.value = true;
    setTimeout(() => {
      saveSuccess.value = false;
    }, 2000);
  } catch (err) {
    console.error('Auto save failed:', err);
  } finally {
    isSaving.value = false;
  }
};

watch(agentConfig, () => {
  if (isFirstLoad) {
    isFirstLoad = false;
    return;
  }
  if (saveTimeout) {
    clearTimeout(saveTimeout);
  }
  saveTimeout = setTimeout(() => {
    autoSave();
  }, 800);
}, { deep: true });

const handleDelete = () => {
  if (confirm('确定要删除这个 Agent 吗？此操作不可撤销。')) {
    emit('delete', agentConfig.value.id);
  }
};

onMounted(async () => {
  if (props.id) {
    try {
      const config = await invoke<AgentConfig>('read_agent_config', { 
        agentId: props.id, 
        allowDefault: true 
      });
      agentConfig.value = config;
    } catch (err) {
      console.error('Failed to load agent config:', err);
    }
  }
});
</script>

<template>
  <div class="agent-settings-view h-full flex flex-col bg-secondary-bg text-primary-text">
    
    <!-- Header -->
    <header class="p-3 flex items-center justify-between border-b border-black/10 dark:border-white/10 pt-[calc(var(--vcp-safe-top,24px)+10px)] pb-3 shrink-0 bg-black/5 dark:bg-white/5">
      <div class="flex items-center gap-2">
        <button @click="$router.back()" class="p-2 hover:bg-black/5 dark:hover:bg-white/10 rounded-lg active:scale-95 transition-all">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="19" y1="12" x2="5" y2="12"></line><polyline points="12 19 5 12 12 5"></polyline></svg>
        </button>
        <h2 class="text-base font-bold">助手设置</h2>
      </div>
      <div class="text-xs font-bold transition-opacity duration-300" :class="{ 'opacity-100': isSaving || saveSuccess, 'opacity-0': !isSaving && !saveSuccess }">
        <span v-if="isSaving" class="text-blue-400 animate-pulse">保存中...</span>
        <span v-else-if="saveSuccess" class="text-green-500">已自动保存 ✅</span>
      </div>
    </header>

    <!-- Scrollable Form Area -->
    <div class="flex-1 overflow-y-auto p-5 space-y-6 pb-safe">
      
      <!-- 1. Identity Section -->
      <section class="card-modern">
        <div class="flex flex-col items-center gap-6">
          <div class="relative group">
            <div class="w-24 h-24 rounded-3xl bg-black/5 dark:bg-white/5 flex-center relative overflow-hidden border-2 border-dashed border-black/10 dark:border-white/20 shadow-inner group-active:scale-95 transition-all">
              <img v-if="agentConfig.avatar" :src="agentConfig.avatar" class="w-full h-full object-cover" />
              <div v-else class="flex flex-col items-center opacity-30">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"></path><circle cx="12" cy="13" r="4"></circle></svg>
              </div>
              <div class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 flex-center transition-opacity cursor-pointer">
                <span class="text-[10px] text-white font-bold tracking-widest uppercase">更换头像</span>
              </div>
            </div>
            <input type="file" class="hidden" accept="image/*" />
          </div>
          
          <div class="w-full">
            <label class="text-[11px] uppercase font-black tracking-widest opacity-40 dark:opacity-30 mb-2 block text-center">Agent 名称</label>
            <input v-model="agentConfig.name" placeholder="为你的助手起个名字..." class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 w-full rounded-2xl focus:border-blue-500/50 outline-none py-3.5 px-4 text-center text-lg font-bold transition-all text-primary-text" />
          </div>
        </div>
      </section>

      <!-- 2. System Prompt Section -->
      <section class="space-y-3">
        <div class="flex items-center gap-2 px-2 py-1">
          <div class="w-1 h-4 bg-purple-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">系统提示词 (System Prompt)</h3>
        </div>
        <div class="card-modern">
          <textarea v-model="agentConfig.systemPrompt" placeholder="在这里输入助手的核心指令..." class="w-full bg-black/5 dark:bg-white/5 rounded-2xl p-4 text-sm outline-none min-h-[150px] resize-none focus:bg-black/10 transition-all leading-relaxed"></textarea>
          <p class="mt-3 text-[10px] opacity-30 px-1 leading-normal">提示：系统提示词定义了助手的性格、知识范围和行为准则。</p>
        </div>
      </section>

      <!-- 3. Style Settings (Collapsible) -->
      <section class="space-y-3">
        <button @click="toggleSection('style')" class="w-full flex items-center justify-between px-2 py-1">
          <div class="flex items-center gap-2">
            <div class="w-1 h-4 bg-pink-500 rounded-full"></div>
            <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">自定义样式</h3>
          </div>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-300" :class="{ 'rotate-180': sections.style }"><polyline points="6 9 12 15 18 9"></polyline></svg>
        </button>
        
        <div v-if="sections.style" class="card-modern space-y-5 animate-in fade-in slide-in-from-top-2 duration-300">
          <div class="space-y-4">
            <div class="flex justify-between items-center">
              <span class="text-sm font-medium">使用主题默认颜色</span>
              <label class="relative inline-flex items-center cursor-pointer">
                <input type="checkbox" v-model="agentConfig.disableCustomColors" class="sr-only peer">
                <div class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-pink-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5"></div>
              </label>
            </div>
            
            <div class="flex justify-between items-center">
              <span class="text-sm font-medium">会话中使用主题默认颜色</span>
              <label class="relative inline-flex items-center cursor-pointer">
                <input type="checkbox" v-model="agentConfig.useThemeColorsInChat" class="sr-only peer">
                <div class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-pink-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5"></div>
              </label>
            </div>
            
            <div class="grid grid-cols-2 gap-4">
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">外框颜色</label>
                <div class="flex gap-2">
                  <input type="color" v-model="agentConfig.avatarBorderColor" class="w-10 h-10 rounded-lg border-none bg-transparent" />
                  <input v-model="agentConfig.avatarBorderColor" class="flex-1 bg-black/5 dark:bg-white/5 rounded-lg px-2 text-xs font-mono outline-none" />
                </div>
              </div>
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">名称颜色</label>
                <div class="flex gap-2">
                  <input type="color" v-model="agentConfig.nameTextColor" class="w-10 h-10 rounded-lg border-none bg-transparent" />
                  <input v-model="agentConfig.nameTextColor" class="flex-1 bg-black/5 dark:bg-white/5 rounded-lg px-2 text-xs font-mono outline-none" />
                </div>
              </div>
            </div>

            <div class="space-y-4 pt-2">
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">列表项自定义 CSS</label>
                <textarea v-model="agentConfig.customCss" placeholder="e.g. border-radius: 10px;" class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-xs font-mono outline-none min-h-[60px] resize-none focus:bg-black/10 transition-all"></textarea>
              </div>
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">会话样式 CSS</label>
                <textarea v-model="agentConfig.chatCss" placeholder="e.g. .message-avatar { filter: blur(2px); }" class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-xs font-mono outline-none min-h-[60px] resize-none focus:bg-black/10 transition-all"></textarea>
              </div>
            </div>
          </div>
        </div>
      </section>

      <!-- 3. Model Parameters (Collapsible) -->
      <section class="space-y-3">
        <button @click="toggleSection('params')" class="w-full flex items-center justify-between px-2 py-1">
          <div class="flex items-center gap-2">
            <div class="w-1 h-4 bg-blue-500 rounded-full"></div>
            <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">模型参数配置</h3>
          </div>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-300" :class="{ 'rotate-180': sections.params }"><polyline points="6 9 12 15 18 9"></polyline></svg>
        </button>
        
        <div v-if="sections.params" class="card-modern space-y-5 animate-in fade-in slide-in-from-top-2 duration-300">
          <div>
            <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">模型名称</label>
            <div class="flex gap-2">
              <input v-model="agentConfig.model" class="flex-1 bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none focus:bg-black/10 transition-all font-mono" />
              <button @click="showModelSelector = true" class="w-12 h-12 bg-blue-500/10 text-blue-500 rounded-xl flex-center active:scale-90 transition-all">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path></svg>
              </button>
            </div>
          </div>

          <div class="grid grid-cols-2 gap-5">
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">Top P ({{ agentConfig.top_p }})</label>
              <input type="range" v-model.number="agentConfig.top_p" min="0" max="1" step="0.05" class="w-full accent-blue-500" />
            </div>
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">Top K ({{ agentConfig.top_k }})</label>
              <input type="number" v-model.number="agentConfig.top_k" min="0" max="64" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
            </div>
          </div>

          <div class="grid grid-cols-2 gap-5">
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">上下文 Token 上限</label>
              <input type="number" v-model.number="agentConfig.contextTokenLimit" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
            </div>
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">最大输出 Token</label>
              <input type="number" v-model.number="agentConfig.maxOutputTokens" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
            </div>
          </div>
          
          <div class="flex justify-between items-center py-2">
            <span class="text-sm font-medium">流式输出</span>
            <label class="relative inline-flex items-center cursor-pointer">
              <input type="checkbox" v-model="agentConfig.streamOutput" class="sr-only peer">
              <div class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-blue-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5"></div>
            </label>
          </div>
        </div>
      </section>

      <!-- 4. TTS Settings (Collapsible) -->
      <section class="space-y-3">
        <button @click="toggleSection('tts')" class="w-full flex items-center justify-between px-2 py-1">
          <div class="flex items-center gap-2">
            <div class="w-1 h-4 bg-green-500 rounded-full"></div>
            <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">语音设置 (Sovits)</h3>
          </div>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-300" :class="{ 'rotate-180': sections.tts }"><polyline points="6 9 12 15 18 9"></polyline></svg>
        </button>
        
        <div v-if="sections.tts" class="card-modern space-y-5 animate-in fade-in slide-in-from-top-2 duration-300">
          <div class="space-y-4">
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">主语言模型</label>
              <select v-model="agentConfig.ttsVoicePrimary" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none appearance-none">
                <option value="">不使用语音</option>
                <!-- Add options here -->
              </select>
            </div>
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">主语言正则</label>
              <input v-model="agentConfig.ttsRegexPrimary" placeholder="匹配全部: .*" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
            </div>

            <div class="border-t border-white/5 pt-4">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">副语言模型</label>
              <select v-model="agentConfig.ttsVoiceSecondary" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none appearance-none">
                <option value="">不使用</option>
                <!-- Add options here -->
              </select>
            </div>
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">副语言正则</label>
              <input v-model="agentConfig.ttsRegexSecondary" placeholder="e.g. \[(.*?)\]" class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
            </div>

            <div class="border-t border-white/5 pt-4">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">语速 ({{ agentConfig.ttsSpeed.toFixed(1) }}x)</label>
              <input type="range" v-model.number="agentConfig.ttsSpeed" min="0.5" max="2.0" step="0.1" class="w-full accent-green-500" />
            </div>
          </div>
        </div>
      </section>

      <div class="h-4"></div>

      <!-- Actions -->
      <div class="space-y-4">
        <button @click="handleDelete" class="w-full py-3 bg-transparent border border-red-500/20 text-red-500/60 hover:bg-red-500/5 active:bg-red-500/10 active:scale-95 transition-all rounded-xl font-bold uppercase tracking-widest text-[11px]">
          删除此 Agent
        </button>
      </div>

    </div>

    <!-- 模型选择器 -->
    <ModelSelector 
      v-model="showModelSelector" 
      :current-model="agentConfig.model"
      title="选择助手模型"
      @select="onModelSelect"
    />
  </div>
</template>

<style scoped>
.agent-settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
}

.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-xl p-4 shadow-sm;
}

input[type="number"]::-webkit-inner-spin-button,
input[type="number"]::-webkit-outer-spin-button {
  -webkit-appearance: none;
  margin: 0;
}

.flex-center {
  @apply flex items-center justify-center;
}

/* Custom switch style if needed, though standard peer-checked works well */
</style>
