<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue';
import { useTarvenStore, type TarvenRule } from '../../../core/stores/tarvenStore';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import SlidePage from '../../../components/ui/SlidePage.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';

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

// 自定义删除确认弹窗状态
const showDeleteConfirm = ref(false);
const ruleIdToDelete = ref<string | null>(null);

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

// 触发优雅的删除确认弹窗
const confirmDelete = (id: string) => {
  ruleIdToDelete.value = id;
  showDeleteConfirm.value = true;
};

const executeDelete = async () => {
  if (ruleIdToDelete.value) {
    await tarvenStore.deleteRule(ruleIdToDelete.value);
  }
  showDeleteConfirm.value = false;
  ruleIdToDelete.value = null;
};

const cancelDelete = () => {
  showDeleteConfirm.value = false;
  ruleIdToDelete.value = null;
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

const enableSystemMetadata = computed({
  get: () => tarvenStore.rules.find(r => r.id === 'system_meta_injection')?.isEnabled ?? true,
  set: () => tarvenStore.toggleRule('system_meta_injection')
});

const enableTimeAnchoring = computed({
  get: () => tarvenStore.rules.find(r => r.id === 'time_anchoring_v2')?.isEnabled ?? false,
  set: () => tarvenStore.toggleRule('time_anchoring_v2')
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
    <div class="tarven-settings flex flex-col h-full w-full bg-[var(--primary-bg)] text-[var(--primary-text)] pointer-events-auto">
      
      <!-- 头部 -->
      <header class="px-4 py-3 flex items-center justify-between border-b border-black/10 dark:border-white/5 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0">
        <div class="flex items-center gap-2">
          <button @click="handleBack" class="p-2 -ml-2 active:scale-90 transition-transform opacity-75 active:opacity-100 flex items-center justify-center text-[var(--primary-text)]">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="m15 18-6-6 6-6"/>
            </svg>
          </button>
          <h2 class="text-[16px] font-bold tracking-tight">
            {{ currentView === 'form' ? (editingRule.id ? '编辑注入预设' : '创建注入预设') : '注入预设' }}
          </h2>
        </div>
      </header>

      <!-- 滚动区域 -->
      <div class="flex-1 overflow-y-auto relative no-rubber-band px-4 py-4 scrollbar-none">
        
        <!-- 视图 A: 规则列表 (按类型分区展示与排序) -->
        <div v-if="currentView === 'list'" class="flex flex-col gap-4 animate-fade-in pb-16">
          
          <!-- 内置高级注入开关卡片 -->
          <div class="flex flex-col p-4 rounded-2xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 gap-3.5">
            <div class="flex items-center gap-2 border-b border-black/5 dark:border-white/5 pb-2">
              <div class="w-1.5 h-4 bg-emerald-500 rounded-full"></div>
              <span class="text-[12.5px] font-extrabold text-[var(--primary-text)] tracking-wide">系统内置高级注入开关</span>
            </div>

            <!-- 系统环境元数据注入 -->
            <div class="flex items-center justify-between">
              <div class="flex flex-col min-w-0 pr-4">
                <span class="text-[13px] font-bold text-[var(--primary-text)]">系统环境元数据注入</span>
                <span class="text-[10.5px] text-[var(--secondary-text)] mt-0.5 leading-normal font-medium">
                  流式请求时在 System Prompt 顶部自动注入系统时间、运行环境及话题创建时间。
                </span>
              </div>
              <SettingsSwitch v-model="enableSystemMetadata" class="shrink-0" />
            </div>

            <!-- 时间锚定机制 V2 (重命名为 消息时间线感知) -->
            <div class="flex items-center justify-between border-t border-black/5 dark:border-white/5 pt-3.5">
              <div class="flex flex-col min-w-0 pr-4">
                <span class="text-[13px] font-bold text-[var(--primary-text)]">消息时间线感知</span>
                <span class="text-[10.5px] text-[var(--secondary-text)] mt-0.5 leading-normal font-medium">
                  为上下文中每条消息注入发送时间戳，使大模型具备精确的时间线感知，防止其对物理时间产生幻觉。
                </span>
              </div>
              <SettingsSwitch v-model="enableTimeAnchoring" class="shrink-0" />
            </div>
          </div>

          <!-- 创建新规则虚线按钮 -->
          <button @click="openForm()"
            class="flex items-center justify-center gap-2 p-3.5 rounded-xl border border-dashed border-black/15 dark:border-white/15 text-[var(--secondary-text)] hover:text-[var(--primary-text)] active:scale-[0.98] transition-all bg-black/5 dark:bg-white/5 w-full">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"/><path d="M12 8v8"/><path d="M8 12h8"/>
            </svg>
            <span class="text-[13px] font-bold">创建自定义注入规则</span>
          </button>

          <!-- 1. 系统提示词尾部注入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.system_suffix = !collapsedSections.system_suffix"
              class="flex items-center justify-between py-2.5 px-3 rounded-xl bg-primary/5 dark:bg-primary/10 text-[12px] font-bold text-primary border border-primary/20 active:scale-[0.99] transition-all">
              <div class="flex items-center gap-2">
                <div class="w-1 h-3.5 bg-primary rounded-full"></div>
                <span>系统提示词注入</span>
                <span class="text-[10px] text-primary/70 dark:text-primary/80 font-mono">({{ systemRules.length }})</span>
              </div>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.system_suffix ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.system_suffix" class="flex flex-col gap-2 mt-1">
              <div v-for="(rule, idx) in systemRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 relative group transition-all cursor-pointer active:scale-[0.99]"
                :class="{ '!border-emerald-500/60 border-2 bg-emerald-500/8 shadow-[0_0_15px_-3px_rgba(16,185,129,0.15)]': rule.isEnabled }"
                @click="handleToggle(rule.id)">
                <div class="flex items-center justify-between">
                  <div class="flex flex-col min-w-0 pr-4">
                    <span class="text-[14px] font-bold text-[var(--primary-text)] truncate transition-colors"
                      :class="{ 'text-emerald-500 dark:text-emerald-400 font-black': rule.isEnabled }">{{ rule.name }}</span>
                    
                    <div class="flex flex-wrap items-center gap-1.5 mt-1.5">
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold uppercase tracking-wider bg-blue-500/10 text-blue-500 border border-blue-500/20">
                        系统提示词
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
                        {{ rule.scope === 'global' ? '全局' : rule.scope === 'agent' ? '智能体' : '群组' }}
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700 uppercase">
                        {{ rule.position === 'prepend' ? '前置' : '后置' }}
                      </span>
                    </div>
                  </div>

                  <div class="flex items-center gap-1 shrink-0" @click.stop>
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === systemRules.length - 1"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="confirmDelete(rule.id)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-danger active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
              </div>
              <div v-if="systemRules.length === 0" class="text-[11px] text-[var(--secondary-text)] py-3 text-center border border-dashed border-black/10 dark:border-white/10 rounded-xl">
                无激活的系统提示词注入项
              </div>
            </div>
          </div>

          <!-- 2. 用户消息注入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.user_suffix = !collapsedSections.user_suffix"
              class="flex items-center justify-between py-2.5 px-3 rounded-xl bg-primary/5 dark:bg-primary/10 text-[12px] font-bold text-primary border border-primary/20 active:scale-[0.99] transition-all">
              <div class="flex items-center gap-2">
                <div class="w-1 h-3.5 bg-primary rounded-full"></div>
                <span>用户消息注入</span>
                <span class="text-[10px] text-primary/70 dark:text-primary/80 font-mono">({{ userRules.length }})</span>
              </div>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.user_suffix ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.user_suffix" class="flex flex-col gap-2 mt-1">
              <div v-for="(rule, idx) in userRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 relative group transition-all cursor-pointer active:scale-[0.99]"
                :class="{ '!border-emerald-500/60 border-2 bg-emerald-500/8 shadow-[0_0_15px_-3px_rgba(16,185,129,0.15)]': rule.isEnabled }"
                @click="handleToggle(rule.id)">
                <div class="flex items-center justify-between">
                  <div class="flex flex-col min-w-0 pr-4">
                    <span class="text-[14px] font-bold text-[var(--primary-text)] truncate transition-colors"
                      :class="{ 'text-emerald-500 dark:text-emerald-400 font-black': rule.isEnabled }">{{ rule.name }}</span>
                    
                    <div class="flex flex-wrap items-center gap-1.5 mt-1.5">
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold uppercase tracking-wider bg-emerald-500/10 text-emerald-500 border border-emerald-500/20">
                        用户消息
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
                        {{ rule.scope === 'global' ? '全局' : rule.scope === 'agent' ? '智能体' : '群组' }}
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700 uppercase">
                        {{ rule.position === 'prepend' ? '前置' : '后置' }}
                      </span>
                    </div>
                  </div>

                  <div class="flex items-center gap-1 shrink-0" @click.stop>
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === userRules.length - 1"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="confirmDelete(rule.id)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-danger active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
              </div>
              <div v-if="userRules.length === 0" class="text-[11px] text-[var(--secondary-text)] py-3 text-center border border-dashed border-black/10 dark:border-white/10 rounded-xl">
                无激活的用户消息注入项
              </div>
            </div>
          </div>

          <!-- 3. 上下文消息注入分区 -->
          <div class="category-section flex flex-col gap-2">
            <button @click="collapsedSections.context_inject = !collapsedSections.context_inject"
              class="flex items-center justify-between py-2.5 px-3 rounded-xl bg-primary/5 dark:bg-primary/10 text-[12px] font-bold text-primary border border-primary/20 active:scale-[0.99] transition-all">
              <div class="flex items-center gap-2">
                <div class="w-1 h-3.5 bg-primary rounded-full"></div>
                <span>上下文消息注入</span>
                <span class="text-[10px] text-primary/70 dark:text-primary/80 font-mono">({{ contextRules.length }})</span>
              </div>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform duration-200" :class="collapsedSections.context_inject ? '-rotate-90' : ''">
                <path d="m6 9 6 6 6-6"/>
              </svg>
            </button>
            
            <div v-show="!collapsedSections.context_inject" class="flex flex-col gap-2 mt-1">
              <div v-for="(rule, idx) in contextRules" :key="rule.id"
                class="rule-card flex flex-col p-3.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 relative group transition-all cursor-pointer active:scale-[0.99]"
                :class="{ '!border-emerald-500/60 border-2 bg-emerald-500/8 shadow-[0_0_15px_-3px_rgba(16,185,129,0.15)]': rule.isEnabled }"
                @click="handleToggle(rule.id)">
                <div class="flex items-center justify-between">
                  <div class="flex flex-col min-w-0 pr-4">
                    <span class="text-[14px] font-bold text-[var(--primary-text)] truncate transition-colors"
                      :class="{ 'text-emerald-500 dark:text-emerald-400 font-black': rule.isEnabled }">{{ rule.name }}</span>
                    
                    <div class="flex flex-wrap items-center gap-1.5 mt-1.5">
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold uppercase tracking-wider bg-orange-500/10 text-orange-500 border border-orange-500/20">
                        上下文注入
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
                        {{ rule.scope === 'global' ? '全局' : rule.scope === 'agent' ? '智能体' : '群组' }}
                      </span>
                      <span class="px-2 py-0.5 rounded-md text-[9px] font-bold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
                        {{ rule.role === 'user' ? '用户角色' : '智能体' }} · 深度 {{ rule.depth }}
                      </span>
                    </div>
                  </div>

                  <div class="flex items-center gap-1 shrink-0" @click.stop>
                    <button @click="handleMove(rule, 'up')" :disabled="idx === 0"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m18 15-6-6-6 6"/></svg>
                    </button>
                    <button @click="handleMove(rule, 'down')" :disabled="idx === contextRules.length - 1"
                      class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] disabled:opacity-10 active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <button @click="openForm(rule)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-[var(--primary-text)] active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                    </button>
                    <button @click="confirmDelete(rule.id)" class="p-1.5 rounded-lg text-[var(--secondary-text)] hover:text-danger active:scale-90 transition-all">
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                    </button>
                  </div>
                </div>
              </div>
              <div v-if="contextRules.length === 0" class="text-[11px] text-[var(--secondary-text)] py-3 text-center border border-dashed border-black/10 dark:border-white/10 rounded-xl">
                无激活的上下文消息注入项
              </div>
            </div>
          </div>
        </div>

        <!-- 视图 B: 表单配置与实时所见即所得 JSON 预览页 -->
        <div v-else class="flex flex-col gap-4 animate-fade-in pb-24">
          
          <!-- 规则基础设置 -->
          <div class="flex flex-col gap-4.5 p-4.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10">
            <!-- 名称 -->
            <div class="flex flex-col gap-1.5">
              <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">规则名称</label>
              <input v-model="editingRule.name" type="text" placeholder=""
                class="w-full px-3.5 py-3 rounded-xl bg-black/5 dark:bg-black/20 border border-black/10 dark:border-white/10 focus:outline-none focus:border-primary/50 focus:ring-2 focus:ring-primary/10 text-[13px] font-bold text-[var(--primary-text)] transition-all" />
            </div>

            <!-- 注入类型 ( Segmented Capsule ) -->
            <div class="flex flex-col gap-2">
              <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">注入类型</label>
              <div class="flex bg-black/10 dark:bg-white/5 p-1 rounded-xl gap-1">
                <button type="button" @click="editingRule.ruleType = 'system_suffix'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.ruleType === 'system_suffix'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <rect width="18" height="18" x="3" y="3" rx="2"/><path d="M21 9H3M21 15H3M12 3v18"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">系统提示词注入</span>
                </button>
                
                <button type="button" @click="editingRule.ruleType = 'user_suffix'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.ruleType === 'user_suffix'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">用户消息注入</span>
                </button>

                <button type="button" @click="editingRule.ruleType = 'context_inject'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.ruleType === 'context_inject'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text(--primary-text) font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 8v8M8 12h8"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">上下文消息注入</span>
                </button>
              </div>
            </div>

            <!-- 作用范围 ( Segmented Capsule ) -->
            <div class="flex flex-col gap-2">
              <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">作用范围</label>
              <div class="flex bg-black/10 dark:bg-white/5 p-1 rounded-xl gap-1">
                <button type="button" @click="editingRule.scope = 'global'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.scope === 'global'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">全局生效</span>
                </button>
                
                <button type="button" @click="editingRule.scope = 'agent'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.scope === 'agent'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">仅单聊</span>
                </button>

                <button type="button" @click="editingRule.scope = 'group'"
                  class="flex-1 flex flex-col items-center justify-center gap-1 py-2 px-1 rounded-lg transition-all text-center"
                  :class="editingRule.scope === 'group'
                    ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                    : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="shrink-0">
                    <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/>
                    <path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/>
                  </svg>
                  <span class="text-[10px] tracking-tight">仅群聊</span>
                </button>
              </div>
            </div>

            <!-- 包裹配置与位置动态项 -->
            <div class="flex flex-wrap items-center justify-between gap-4 border-t border-black/10 dark:border-white/10 pt-3">
              <div class="flex items-center gap-2">
                <input type="checkbox" v-model="editingRule.wrap" id="chk-wrap"
                  class="rounded border-black/15 dark:border-white/15 text-primary focus:ring-primary/20 w-4.5 h-4.5 bg-transparent cursor-pointer accent-primary" />
                <label for="chk-wrap" class="text-[12px] font-bold text-[var(--primary-text)] cursor-pointer">
                  使用 XML 标签包裹注入内容（提升模型兼容性）
                </label>
              </div>
            </div>

            <!-- SYSTEM_SUFFIX / USER_SUFFIX 位置配置 -->
            <div v-if="editingRule.ruleType === 'system_suffix' || editingRule.ruleType === 'user_suffix'" 
              class="flex flex-col gap-1.5 border-t border-black/10 dark:border-white/10 pt-3">
              <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">拼接位置</label>
              <div class="flex gap-4">
                <label class="flex items-center gap-2 text-[12.5px] font-bold cursor-pointer text-[var(--primary-text)]">
                  <input type="radio" v-model="editingRule.position" value="prepend" class="text-primary w-4.5 h-4.5 cursor-pointer bg-transparent border-black/15 dark:border-white/15 accent-primary" />
                  <span>置顶前置</span>
                </label>
                <label class="flex items-center gap-2 text-[12.5px] font-bold cursor-pointer text-[var(--primary-text)]">
                  <input type="radio" v-model="editingRule.position" value="append" class="text-primary w-4.5 h-4.5 cursor-pointer bg-transparent border-black/15 dark:border-white/15 accent-primary" />
                  <span>置底后置</span>
                </label>
              </div>
            </div>

            <!-- CONTEXT_INJECT 消息注入专用参数 -->
            <div v-if="editingRule.ruleType === 'context_inject'" 
              class="flex flex-col gap-3.5 border-t border-black/10 dark:border-white/10 pt-3">
              
              <!-- Role ( Segmented Capsule ) -->
              <div class="flex flex-col gap-2">
                <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">虚拟消息角色</label>
                <div class="flex bg-black/10 dark:bg-white/5 p-1 rounded-xl gap-1">
                  <button type="button" @click="editingRule.role = 'user'"
                    class="flex-1 flex flex-col items-center justify-center gap-1.5 py-2 px-1 rounded-lg transition-all text-center"
                    :class="editingRule.role === 'user'
                      ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                      : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>
                    </svg>
                    <span class="text-[10px] tracking-tight">用户角色</span>
                  </button>

                  <button type="button" @click="editingRule.role = 'assistant'"
                    class="flex-1 flex flex-col items-center justify-center gap-1.5 py-2 px-1 rounded-lg transition-all text-center"
                    :class="editingRule.role === 'assistant'
                      ? 'bg-white dark:bg-white/10 text-primary font-bold shadow-sm'
                      : 'text-[var(--secondary-text)] hover:text-[var(--primary-text)] font-semibold active:scale-95'">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 17h6M9 12h6M9 8h6"/>
                    </svg>
                    <span class="text-[10px] tracking-tight">智能体</span>
                  </button>
                </div>
              </div>

              <!-- Depth -->
              <div class="flex flex-col gap-1.5">
                <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] px-0.5">插入深度</label>
                <div class="flex items-center gap-3">
                  <input type="number" v-model.number="editingRule.depth" min="0" max="20"
                    class="w-20 px-3 py-2 rounded-xl bg-black/10 dark:bg-white/5 border border-black/10 dark:border-white/10 focus:outline-none focus:border-primary/50 text-[13px] font-mono text-center font-bold text-[var(--primary-text)]" />
                  <span class="text-[11px] text-[var(--secondary-text)] font-medium leading-normal">
                    0 = 上下文绝对末尾；N = 倒数第 N+1 条消息之前
                  </span>
                </div>
              </div>
            </div>
          </div>

          <!-- 规则内容输入框 -->
          <div class="flex flex-col gap-2 p-4.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10">
            <label class="text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)]">规则内容</label>
            <textarea v-model="editingRule.content" rows="6" placeholder="输入规则"
              class="w-full px-3.5 py-3 rounded-xl bg-black/5 dark:bg-black/20 border border-black/10 dark:border-white/10 focus:outline-none focus:border-primary/50 focus:ring-2 focus:ring-primary/10 text-[12px] font-medium text-[var(--primary-text)] leading-relaxed resize-none scrollbar-none"></textarea>
          </div>

          <!-- 所见即所得“注入预览”区域 -->
          <div class="flex flex-col gap-2 p-4.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10">
            <button @click="showPreview = !showPreview"
              class="flex items-center justify-between text-[11px] font-bold uppercase tracking-wider text-[var(--secondary-text)] w-full">
              <span>规则预览</span>
              <span class="text-[9px] text-primary font-mono lowercase">
                {{ showPreview ? '点击收起' : '点击展示' }}
              </span>
            </button>

            <div v-show="showPreview" class="flex flex-col gap-3 mt-2">
              <div v-if="isPreviewLoading" class="flex items-center justify-center py-6 text-[var(--secondary-text)]">
                <svg class="animate-spin -ml-1 mr-2 h-4 w-4 text-[var(--secondary-text)]" fill="none" viewBox="0 0 24 24">
                  <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                  <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
                </svg>
                <span class="text-[11px]">正在渲染实时预览...</span>
              </div>
              
              <div v-else-if="previewMessages.length > 0" class="flex flex-col gap-2 bg-black/5 dark:bg-black/25 p-3 rounded-xl border border-black/10 dark:border-white/5 font-mono text-[10px] overflow-y-auto max-h-[250px] scrollbar-none leading-relaxed">
                <div v-for="(msg, index) in previewMessages" :key="index"
                  class="flex flex-col p-2.5 rounded-lg bg-black/5 dark:bg-white/5 border transition-all"
                  :class="msg.__tavernInjected ? 'border-dashed border-primary/50 bg-primary/5' : 'border-black/5 dark:border-white/5'">
                  <div class="flex items-center justify-between pb-1 mb-1 border-b border-black/5 dark:border-white/5 opacity-75">
                    <span class="font-bold uppercase tracking-wider text-[9px]" :class="msg.role === 'system' ? 'text-purple-500' : msg.role === 'user' ? 'text-primary' : 'text-success'">
                      [{{ index }}] {{ msg.role }}
                    </span>
                    <span v-if="msg.__tavernInjected" class="text-[8px] bg-primary/20 text-primary px-1 py-0.5 rounded font-sans uppercase font-bold tracking-wide">
                      XML 格式注入
                    </span>
                  </div>
                  <pre class="whitespace-pre-wrap break-all select-text font-medium opacity-90 text-[var(--primary-text)]">{{ msg.content }}</pre>
                </div>
              </div>
              
              <div v-else class="text-[11px] text-[var(--placeholder-text)] py-6 text-center">
                输入预设名和内容后，此处将自动呈现预览
              </div>
            </div>
          </div>

          <!-- 保存按钮 -->
          <button @click="handleSave" :disabled="!editingRule.name || !editingRule.content"
            class="w-full mt-2 py-3.5 rounded-xl bg-primary disabled:bg-black/10 dark:disabled:bg-white/5 text-white disabled:text-[var(--secondary-text)] text-[13px] font-bold shadow-lg shadow-primary/10 active:scale-[0.98] transition-all flex items-center justify-center">
            保存规则并应用到数据库
          </button>
        </div>
      </div>
    </div>

    <!-- 独立的自定义优雅删除确认弹窗 -->
    <Teleport to="body">
      <Transition name="fade">
        <div v-if="showDeleteConfirm" 
          class="fixed inset-0 bg-black/70 z-dialog flex items-center justify-center p-6"
          @click="cancelDelete"
          @touchmove.prevent>
          
          <div class="bg-[var(--secondary-bg)] border border-black/10 dark:border-white/10 rounded-2xl w-full max-w-[280px] p-5 shadow-2xl flex flex-col text-center"
            @click.stop>
            <div class="w-11 h-11 rounded-full bg-danger/10 text-danger flex items-center justify-center mx-auto mb-3.5">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
            </div>
            
            <h3 class="text-[15px] font-bold text-[var(--primary-text)]">确认删除该预设吗？</h3>
            <p class="text-[11.5px] text-[var(--secondary-text)] mt-2 leading-relaxed font-medium">
              此操作将无法撤销，并会从本地数据库中永久移除该项注入预设。
            </p>
            
            <div class="grid grid-cols-2 gap-2 mt-5 shrink-0">
              <button @click="cancelDelete"
                class="py-2.5 rounded-xl border border-black/10 dark:border-white/10 text-[12px] font-bold bg-transparent text-[var(--secondary-text)] active:scale-95 transition-all">
                取消
              </button>
              <button @click="executeDelete"
                class="py-2.5 rounded-xl bg-danger hover:bg-danger/80 text-white text-[12px] font-bold active:scale-95 transition-all shadow-md shadow-danger/10">
                确认删除
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </SlidePage>
</template>

<style scoped>
.tarven-settings {
  background-color: var(--primary-bg);
}

.animate-fade-in {
  animation: fadeIn 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(6px); }
  to { opacity: 1; transform: translateY(0); }
}

pre {
  margin: 0;
  font-family: inherit;
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
