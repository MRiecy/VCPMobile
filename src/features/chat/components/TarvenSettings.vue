<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue';
import { useTarvenStore, type TarvenRule } from '../../../core/stores/tarvenStore';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import SlidePage from '../../../components/ui/SlidePage.vue';

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: 50,
  }
);

const emit = defineEmits<{
  close: [];
}>();

const tarvenStore = useTarvenStore();
const { registerModal, unregisterModal } = useModalHistory();

const FORM_MODAL_ID = 'TarvenFormPage';

// 视图控制：'list' 列表页, 'form' 新建/编辑页
const currentView = ref<'list' | 'form'>('list');
const editingRule = ref<Partial<TarvenRule>>({});

// 折叠状态记录
const collapsedSections = ref({
  system_suffix: false,
  user_suffix: false,
  context_inject: false,
});

// 分类筛选列表
const systemRules = computed(() => 
  tarvenStore.rules.filter(r => r.ruleType === 'system_suffix').sort((a, b) => a.sortOrder - b.sortOrder)
);

const userRules = computed(() => 
  tarvenStore.rules.filter(r => r.ruleType === 'user_suffix').sort((a, b) => a.sortOrder - b.sortOrder)
);

const contextRules = computed(() => 
  tarvenStore.rules.filter(r => r.ruleType === 'context_inject').sort((a, b) => a.sortOrder - b.sortOrder)
);

// ---------------------------------------------------------
// 注入实时预览机制 (WYSIWYG)
// ---------------------------------------------------------
const previewMessages = ref<any[]>([]);
const isPreviewLoading = ref(false);
const showPreview = ref(true);

const updatePreview = async () => {
  if (!editingRule.value.name || !editingRule.value.content) {
    previewMessages.value = [];
    return;
  }
  
  isPreviewLoading.value = true;
  try {
    const draftRule: TarvenRule = {
      id: editingRule.value.id || 'draft',
      name: editingRule.value.name || '草稿规则',
      ruleType: editingRule.value.ruleType || 'system_suffix',
      isEnabled: true,
      content: editingRule.value.content || '',
      scope: editingRule.value.scope || 'global',
      wrap: editingRule.value.wrap !== false,
      role: editingRule.value.role || 'user',
      depth: editingRule.value.depth ?? 0,
      position: editingRule.value.position || 'append',
      sortOrder: 0,
    };
    
    const res = await tarvenStore.previewInjection([draftRule]);
    previewMessages.value = res;
  } catch (e) {
    console.error('Failed to generate preview:', e);
  } finally {
    isPreviewLoading.value = false;
  }
};

// 监听表单项以进行实时预览更新
watch(
  () => [
    editingRule.value.name,
    editingRule.value.content,
    editingRule.value.ruleType,
    editingRule.value.scope,
    editingRule.value.wrap,
    editingRule.value.role,
    editingRule.value.depth,
    editingRule.value.position,
  ],
  () => {
    if (currentView.value === 'form') {
      updatePreview();
    }
  },
  { deep: true }
);

// ---------------------------------------------------------
// 核心操作
// ---------------------------------------------------------
const openForm = (rule?: TarvenRule) => {
  if (rule) {
    editingRule.value = { ...rule };
  } else {
    editingRule.value = { 
      name: '', 
      ruleType: 'system_suffix',
      content: '', 
      isEnabled: true,
      scope: 'global',
      wrap: true,
      role: 'user',
      depth: 0,
      position: 'append',
      sortOrder: tarvenStore.rules.length
    };
  }
  currentView.value = 'form';
  updatePreview();
};

const closeForm = () => {
  currentView.value = 'list';
  editingRule.value = {};
  previewMessages.value = [];
};

const handleSave = async () => {
  const { id, name, ruleType, content, isEnabled, scope, wrap, role, depth, position, sortOrder } = editingRule.value;
  if (!name || !content || !ruleType) return;

  const ruleData: TarvenRule = {
    id: id || `rule_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
    name,
    ruleType: ruleType as any,
    content,
    isEnabled: isEnabled !== false,
    scope: scope || 'global',
    wrap: wrap !== false,
    role: role || 'user',
    depth: depth ?? 0,
    position: position || 'append',
    sortOrder: sortOrder ?? 0,
  };

  await tarvenStore.saveRule(ruleData);
  closeForm();
};

const handleDelete = async (id: string) => {
  await tarvenStore.deleteRule(id);
};

const handleToggle = async (id: string) => {
  await tarvenStore.toggleRule(id);
};

// 移动排序
const handleMove = async (rule: TarvenRule, direction: 'up' | 'down') => {
  const sameTypeRules = tarvenStore.rules
    .filter(r => r.ruleType === rule.ruleType)
    .sort((a, b) => a.sortOrder - b.sortOrder);
  
  const index = sameTypeRules.findIndex(r => r.id === rule.id);
  if (index === -1) return;
  
  const targetIndex = direction === 'up' ? index - 1 : index + 1;
  if (targetIndex < 0 || targetIndex >= sameTypeRules.length) return;
  
  // 重新生成排好序的 ID 数组并交换
  const sameTypeIds = sameTypeRules.map(r => r.id);
  const temp = sameTypeIds[index];
  sameTypeIds[index] = sameTypeIds[targetIndex];
  sameTypeIds[targetIndex] = temp;
  
  const otherTypeIds = tarvenStore.rules
    .filter(r => r.ruleType !== rule.ruleType)
    .map(r => r.id);
  
  const finalOrderedIds = [...otherTypeIds, ...sameTypeIds];
  await tarvenStore.saveOrder(finalOrderedIds);
};

const handleBack = () => {
  if (currentView.value === 'form') {
    closeForm();
  } else {
    emit('close');
  }
};

watch(currentView, (val) => {
  if (val === 'form') {
    registerModal(FORM_MODAL_ID, () => {
      closeForm();
    });
  } else {
    unregisterModal(FORM_MODAL_ID);
  }
});

onMounted(() => {
  if (props.isOpen) {
    tarvenStore.fetchRules();
  }
});

watch(() => props.isOpen, (val) => {
  if (val) {
    currentView.value = 'list';
    tarvenStore.fetchRules();
  }
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="tarven-settings flex flex-col h-full w-full bg-[var(--secondary-bg)] text-[var(--primary-text)] pointer-events-auto">
      
      <!-- 头部 -->
      <header class="px-4 py-3 flex items-center justify-between border-b border-white/5 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0">
        <div class="flex items-center gap-2">
          <button @click="handleBack" class="p-2 -ml-2 active:scale-90 transition-transform opacity-70 active:opacity-100 flex items-center justify-center">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="m15 18-6-6 6-6"/>
            </svg>
          </button>
          <h2 class="text-lg font-bold tracking-tight">
            {{ currentView === 'form' ? (editingRule.id ? '编辑规则' : '添加新规则') : 'VCPChatTarven 注入预设' }}
          </h2>
        </div>
      </header>

      <!-- 滚动区域 -->
      <div class="flex-1 overflow-y-auto relative no-rubber-band px-4 py-5 scrollbar-none">
        
        <!-- 视图 A: 规则列表 (按类型分区展示与排序) -->
        <div v-if="currentView === 'list'" class="flex flex-col gap-5 animate-fade-in pb-16">
          
          <!-- 添加新规则虚线按钮 -->
          <button @click="openForm()"
            class="flex items-center justify-center gap-2 p-3.5 rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 active:scale-[0.99] transition-all bg-transparent">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"/><path d="M12 8v8"/><path d="M8 12h8"/>
            </svg>
            <span class="text-[13px] font-bold">创建自定义注入规则</span>
          </button>

          <!-- 1. 系统提示词尾部注入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.system_suffix = !collapsedSections.system_suffix"
              class="flex items-center justify-between py-1 px-1 text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
              <span>系统提示词注入 (SYSTEM_SUFFIX) · {{ systemRules.length }}</span>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.system_suffix ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.system_suffix" class="flex flex-col gap-2.5">
              <div v-for="(rule, idx) in systemRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60 relative group">
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-2.5 min-w-0">
                    <input type="checkbox" :checked="rule.isEnabled" @change="handleToggle(rule.id)"
                      class="rounded border-zinc-300 text-blue-500 focus:ring-blue-500/20 w-4 h-4 bg-transparent cursor-pointer" />
                    <span class="text-[14px] font-bold text-zinc-800 dark:text-zinc-200 truncate">{{ rule.name }}</span>
                    <span class="text-[9px] px-1.5 py-0.5 rounded bg-zinc-200 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 font-mono scale-90 uppercase">
                      {{ rule.position === 'prepend' ? '前置' : '后置' }}
                    </span>
                  </div>

                  <div class="flex items-center gap-1.5">
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === systemRules.length - 1"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1 rounded text-zinc-400 hover:text-zinc-600 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="handleDelete(rule.id)" class="p-1 rounded text-zinc-400 hover:text-red-500 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
                <p class="text-[11px] text-zinc-400 dark:text-zinc-500 mt-2 line-clamp-2 break-all font-medium leading-relaxed">
                  {{ rule.content }}
                </p>
              </div>
              <div v-if="systemRules.length === 0" class="text-[11px] text-zinc-400 py-3 text-center border border-dashed border-zinc-100 dark:border-zinc-900 rounded-xl">
                无激活的系统提示词注入项
              </div>
            </div>
          </div>

          <!-- 2. 用户消息注入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.user_suffix = !collapsedSections.user_suffix"
              class="flex items-center justify-between py-1 px-1 text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
              <span>用户消息注入 (USER_SUFFIX) · {{ userRules.length }}</span>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.user_suffix ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.user_suffix" class="flex flex-col gap-2.5">
              <div v-for="(rule, idx) in userRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60 relative group">
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-2.5 min-w-0">
                    <input type="checkbox" :checked="rule.isEnabled" @change="handleToggle(rule.id)"
                      class="rounded border-zinc-300 text-blue-500 focus:ring-blue-500/20 w-4 h-4 bg-transparent cursor-pointer" />
                    <span class="text-[14px] font-bold text-zinc-800 dark:text-zinc-200 truncate">{{ rule.name }}</span>
                    <span class="text-[9px] px-1.5 py-0.5 rounded bg-zinc-200 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 font-mono scale-90 uppercase">
                      {{ rule.position === 'prepend' ? '前置' : '后置' }}
                    </span>
                  </div>

                  <div class="flex items-center gap-1.5">
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === userRules.length - 1"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1 rounded text-zinc-400 hover:text-zinc-600 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="handleDelete(rule.id)" class="p-1 rounded text-zinc-400 hover:text-red-500 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
                <p class="text-[11px] text-zinc-400 dark:text-zinc-500 mt-2 line-clamp-2 break-all font-medium leading-relaxed">
                  {{ rule.content }}
                </p>
              </div>
              <div v-if="userRules.length === 0" class="text-[11px] text-zinc-400 py-3 text-center border border-dashed border-zinc-100 dark:border-zinc-900 rounded-xl">
                无激活的用户消息注入项
              </div>
            </div>
          </div>

          <!-- 3. 独立上下文消息插入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.context_inject = !collapsedSections.context_inject"
              class="flex items-center justify-between py-1 px-1 text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
              <span>独立消息节点注入 (CONTEXT_INJECT) · {{ contextRules.length }}</span>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.context_inject ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.context_inject" class="flex flex-col gap-2.5">
              <div v-for="(rule, idx) in contextRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60 relative group">
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-2.5 min-w-0">
                    <input type="checkbox" :checked="rule.isEnabled" @change="handleToggle(rule.id)"
                      class="rounded border-zinc-300 text-blue-500 focus:ring-blue-500/20 w-4 h-4 bg-transparent cursor-pointer" />
                    <span class="text-[14px] font-bold text-zinc-800 dark:text-zinc-200 truncate">{{ rule.name }}</span>
                    <span class="text-[9px] px-1.5 py-0.5 rounded bg-zinc-200 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 font-mono scale-90 uppercase">
                      Depth: {{ rule.depth }}
                    </span>
                    <span class="text-[9px] px-1.5 py-0.5 rounded bg-blue-500/10 dark:bg-blue-500/20 text-blue-500 font-mono scale-90 uppercase">
                      {{ rule.role }}
                    </span>
                  </div>

                  <div class="flex items-center gap-1.5">
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === contextRules.length - 1"
                      class="p-1 rounded text-zinc-400 hover:text-zinc-600 disabled:opacity-30 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1 rounded text-zinc-400 hover:text-zinc-600 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="handleDelete(rule.id)" class="p-1 rounded text-zinc-400 hover:text-red-500 active:scale-90 transition-transform">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
                <p class="text-[11px] text-zinc-400 dark:text-zinc-500 mt-2 line-clamp-2 break-all font-medium leading-relaxed">
                  {{ rule.content }}
                </p>
              </div>
              <div v-if="contextRules.length === 0" class="text-[11px] text-zinc-400 py-3 text-center border border-dashed border-zinc-100 dark:border-zinc-900 rounded-xl">
                无激活的独立节点注入项
              </div>
            </div>
          </div>
        </div>

        <!-- 视图 B: 表单配置与实时所见即所得 JSON 预览页 -->
        <div v-else class="flex flex-col gap-4.5 animate-fade-in pb-20">
          
          <!-- 规则基础设置 -->
          <div class="flex flex-col gap-4 p-4 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60">
            <!-- 名称 -->
            <div class="flex flex-col gap-1.5">
              <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">规则名称</label>
              <input v-model="editingRule.name" type="text" placeholder="例如：文言文古风 / Android 系统真理"
                class="w-full px-3.5 py-2.5 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/20 text-[13px] font-bold" />
            </div>

            <!-- 类型选择 -->
            <div class="flex flex-col gap-1.5">
              <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">注入类型</label>
              <select v-model="editingRule.ruleType"
                class="w-full px-3.5 py-2.5 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/20 text-[13px] font-bold text-[var(--primary-text)] appearance-none cursor-pointer">
                <option value="system_suffix">追加到系统提示词 (SYSTEM_SUFFIX)</option>
                <option value="user_suffix">追加到最新用户输入末尾 (USER_SUFFIX)</option>
                <option value="context_inject">独立消息节点插入 (CONTEXT_INJECT)</option>
              </select>
            </div>

            <!-- 作用范围 -->
            <div class="flex flex-col gap-1.5">
              <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">作用范围 (Scope)</label>
              <select v-model="editingRule.scope"
                class="w-full px-3.5 py-2.5 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/20 text-[13px] font-bold text-[var(--primary-text)] appearance-none cursor-pointer">
                <option value="global">全部生效 (Global)</option>
                <option value="agent">仅在智能体单聊生效 (Agent)</option>
                <option value="group">仅在智能体群聊生效 (Group)</option>
              </select>
            </div>

            <!-- 包裹配置与位置动态项 -->
            <div class="flex flex-wrap items-center justify-between gap-4 border-t border-zinc-100 dark:border-zinc-900/50 pt-3">
              <div class="flex items-center gap-2">
                <input type="checkbox" v-model="editingRule.wrap" id="chk-wrap"
                  class="rounded border-zinc-300 text-blue-500 focus:ring-blue-500/20 w-4 h-4 bg-transparent cursor-pointer" />
                <label for="chk-wrap" class="text-[12px] font-bold text-zinc-700 dark:text-zinc-300 cursor-pointer">
                  使用 [VCPMobile] 临时注入标记包裹
                </label>
              </div>
            </div>

            <!-- SYSTEM_SUFFIX / USER_SUFFIX 位置配置 -->
            <div v-if="editingRule.ruleType === 'system_suffix' || editingRule.ruleType === 'user_suffix'" 
              class="flex flex-col gap-1.5 border-t border-zinc-100 dark:border-zinc-900/50 pt-3">
              <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">拼接位置 (Position)</label>
              <div class="flex gap-4">
                <label class="flex items-center gap-2 text-[12px] font-bold cursor-pointer">
                  <input type="radio" v-model="editingRule.position" value="prepend" class="text-blue-500 w-4 h-4 cursor-pointer bg-transparent border-zinc-300" />
                  <span>置顶前置 (Prepend)</span>
                </label>
                <label class="flex items-center gap-2 text-[12px] font-bold cursor-pointer">
                  <input type="radio" v-model="editingRule.position" value="append" class="text-blue-500 w-4 h-4 cursor-pointer bg-transparent border-zinc-300" />
                  <span>置底后置 (Append)</span>
                </label>
              </div>
            </div>

            <!-- CONTEXT_INJECT 消息注入专用参数 -->
            <div v-if="editingRule.ruleType === 'context_inject'" 
              class="flex flex-col gap-3.5 border-t border-zinc-100 dark:border-zinc-900/50 pt-3">
              <div class="flex flex-col gap-1.5">
                <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">虚拟消息角色 (Role)</label>
                <select v-model="editingRule.role"
                  class="w-full px-3.5 py-2 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none text-[13px] font-bold text-[var(--primary-text)]">
                  <option value="user">用户 (User)</option>
                  <option value="assistant">智能体 (Assistant)</option>
                </select>
              </div>

              <div class="flex flex-col gap-1.5">
                <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500 px-0.5">插入深度 (Depth)</label>
                <div class="flex items-center gap-3">
                  <input type="number" v-model.number="editingRule.depth" min="0" max="20"
                    class="w-20 px-3 py-2 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none text-[13px] font-mono text-center font-bold" />
                  <span class="text-[11px] text-zinc-400 dark:text-zinc-500">
                    0 = 上下文绝对末尾；N = 倒数第 N+1 条消息之前
                  </span>
                </div>
              </div>
            </div>
          </div>

          <!-- 规则内容输入框 -->
          <div class="flex flex-col gap-2 p-4 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60">
            <label class="text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500">规则内容 (Content)</label>
            <textarea v-model="editingRule.content" rows="6" placeholder="请在此输入你需要注入的文本，支持使用 {{AgentName}} 占位符引用当前智能体名称..."
              class="w-full px-3.5 py-2.5 rounded-xl bg-zinc-100/50 dark:bg-zinc-850 border border-zinc-200 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/20 text-[12px] font-semibold leading-relaxed resize-none scrollbar-none"></textarea>
          </div>

          <!-- 所见即所得“注入预览”区域 -->
          <div class="flex flex-col gap-2 p-4 rounded-xl bg-zinc-50 dark:bg-zinc-800/10 border border-zinc-100 dark:border-zinc-900/60">
            <button @click="showPreview = !showPreview"
              class="flex items-center justify-between text-[11px] font-black uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
              <span>所见即所得：大模型上下文注入预览</span>
              <span class="text-[9px] text-blue-500 dark:text-blue-400 font-mono lowercase">
                {{ showPreview ? '点击收起' : '点击展示' }}
              </span>
            </button>

            <div v-show="showPreview" class="flex flex-col gap-3 mt-2">
              <div v-if="isPreviewLoading" class="flex items-center justify-center py-6 text-zinc-400">
                <svg class="animate-spin -ml-1 mr-2 h-4 w-4 text-zinc-400" fill="none" viewBox="0 0 24 24">
                  <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                  <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
                </svg>
                <span class="text-[11px]">正在利用 Rust 规则管道渲染预览...</span>
              </div>
              
              <div v-else-if="previewMessages.length > 0" class="flex flex-col gap-2 bg-black/20 dark:bg-black/40 p-3 rounded-xl border border-zinc-100/5 dark:border-zinc-900 font-mono text-[10px] overflow-x-auto max-h-[300px] scrollbar-none leading-relaxed">
                <div v-for="(msg, index) in previewMessages" :key="index"
                  class="flex flex-col p-2.5 rounded-lg bg-zinc-900/40 border"
                  :class="msg.__tavernInjected ? 'border-dashed border-blue-500/50 bg-blue-500/5' : 'border-white/5'">
                  <div class="flex items-center justify-between pb-1 mb-1 border-b border-white/5 opacity-70">
                    <span class="font-black uppercase tracking-wider" :class="msg.role === 'system' ? 'text-purple-400' : msg.role === 'user' ? 'text-blue-400' : 'text-green-400'">
                      [{{ index }}] {{ msg.role }}
                    </span>
                    <span v-if="msg.__tavernInjected" class="text-[8px] bg-blue-500/20 text-blue-400 px-1 py-0.5 rounded font-sans uppercase font-black">
                      VCPMobile 注入
                    </span>
                  </div>
                  <pre class="whitespace-pre-wrap break-all select-text font-medium opacity-90">{{ msg.content }}</pre>
                </div>
              </div>
              
              <div v-else class="text-[11px] text-zinc-400/70 py-6 text-center">
                输入规则名和内容后，此处将自动呈现所见即所得的消息上下文预览
              </div>
            </div>
          </div>

          <!-- 保存按钮 -->
          <button @click="handleSave" :disabled="!editingRule.name || !editingRule.content"
            class="w-full mt-2 py-3 rounded-xl bg-blue-500 disabled:bg-zinc-200 dark:disabled:bg-zinc-800 text-white disabled:text-zinc-400 dark:disabled:text-zinc-500 text-[13px] font-black shadow-lg shadow-blue-500/10 active:scale-[0.99] transition-all flex items-center justify-center">
            保存规则并应用到数据库
          </button>
        </div>
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.tarven-settings {
  background-color: color-mix(in srgb, var(--primary-bg) 100%, transparent);
}

.animate-fade-in {
  animation: fadeIn 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}

pre {
  margin: 0;
  font-family: inherit;
}
</style>
