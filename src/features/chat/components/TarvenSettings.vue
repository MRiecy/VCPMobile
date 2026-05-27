<script setup lang="ts">
import { ref, onMounted, watch } from 'vue';
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

const openForm = (rule?: TarvenRule) => {
  if (rule) {
    editingRule.value = { ...rule };
  } else {
    editingRule.value = { name: '', content: '', enabled: true };
  }
  currentView.value = 'form';
};

const closeForm = () => {
  currentView.value = 'list';
  editingRule.value = {};
};

const handleSave = async () => {
  const { id, name, content, enabled } = editingRule.value;
  if (!name || !content) return;

  if (id) {
    await tarvenStore.updateRule(id, { name, content, enabled });
  } else {
    await tarvenStore.addRule(name, content);
  }
  closeForm();
};

const handleDelete = async (id: string) => {
  await tarvenStore.deleteRule(id);
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
      <header class="px-4 py-3 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0">
        <div class="flex items-center gap-2">
          <button @click="handleBack" class="p-2 -ml-2 active:scale-90 transition-transform opacity-70 active:opacity-100 flex items-center justify-center">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="m15 18-6-6 6-6"/>
            </svg>
          </button>
          <h2 class="text-xl font-bold tracking-tight">
            {{ currentView === 'form' ? (editingRule.id ? '编辑规则' : '添加新规则') : 'Tarven 上下文规则' }}
          </h2>
        </div>
      </header>

      <!-- 滚动区域 -->
      <div class="flex-1 overflow-y-auto relative no-rubber-band px-4 py-6">
        
        <!-- 视图 A: 规则列表 -->
        <div v-if="currentView === 'list'" class="flex flex-col gap-4 animate-fade-in pb-12">
          
          <!-- 添加新规则虚线按钮 -->
          <button @click="openForm()"
            class="flex items-center justify-center gap-2 p-4 rounded-2xl border-2 border-dashed border-zinc-200 dark:border-zinc-800 text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 hover:border-zinc-400 active:scale-[0.98] transition-all bg-transparent">
            <div class="i-heroicons-plus-circle text-xl"></div>
            <span class="text-[14px] font-black tracking-wide">添加新规则</span>
          </button>

          <!-- 规则卡片列表 -->
          <template v-if="tarvenStore.rules.length > 0">
            <div v-for="rule in tarvenStore.rules" :key="rule.id"
              class="flex flex-col p-4 rounded-2xl bg-zinc-50 dark:bg-zinc-800/20 border border-zinc-100 dark:border-zinc-800 relative group overflow-hidden">
              
              <!-- 顶部规则名与操作 -->
              <div class="flex items-center justify-between">
                <div class="flex items-center gap-2">
                  <div class="w-2.5 h-2.5 rounded-full" :class="rule.enabled ? 'bg-blue-500' : 'bg-zinc-300 dark:bg-zinc-600'"></div>
                  <span class="text-[15px] font-black text-zinc-800 dark:text-zinc-100">{{ rule.name }}</span>
                </div>

                <!-- 动作按钮栏 -->
                <div class="flex items-center gap-1">
                  <!-- 编辑 -->
                  <button @click="openForm(rule)"
                    class="p-1.5 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 text-zinc-500 active:scale-90 transition-transform">
                    <div class="i-heroicons-pencil-square text-lg"></div>
                  </button>
                  <!-- 删除 -->
                  <button @click="handleDelete(rule.id)"
                    class="p-1.5 rounded-lg hover:bg-red-500/10 text-zinc-400 hover:text-red-500 active:scale-90 transition-transform">
                    <div class="i-heroicons-trash text-lg"></div>
                  </button>
                </div>
              </div>

              <!-- 规则内容 -->
              <p class="text-[12px] text-zinc-400 dark:text-zinc-500 mt-2 line-clamp-3 leading-relaxed break-all whitespace-pre-wrap">
                {{ rule.content }}
              </p>
            </div>
          </template>
          
          <template v-else>
            <!-- 缺省 -->
            <div class="flex flex-col items-center justify-center py-20 text-center">
              <div class="w-16 h-16 flex items-center justify-center rounded-full bg-zinc-50 dark:bg-zinc-800/40 border border-zinc-100 dark:border-zinc-800 text-zinc-400 mb-4">
                <div class="i-heroicons-circle-stack text-3xl"></div>
              </div>
              <span class="text-[14px] font-bold text-zinc-700 dark:text-zinc-300">暂无任何上下文规则</span>
              <span class="text-[11px] text-zinc-400 mt-1 max-w-[220px]">创建属于您的规则，例如：时区、指定角色设定或特定应用常识</span>
            </div>
          </template>
        </div>

        <!-- 视图 B: 表单页 -->
        <div v-else class="flex flex-col gap-5 animate-fade-in">
          <!-- 规则名称 -->
          <div class="flex flex-col gap-2">
            <label class="text-[11px] font-black uppercase tracking-[0.15em] text-zinc-400 dark:text-zinc-500 px-1">规则名称</label>
            <input v-model="editingRule.name" type="text" placeholder="例如：文言文古风 / Android 极简"
              class="w-full px-4 py-3 rounded-2xl bg-zinc-50 dark:bg-zinc-800/30 border border-zinc-100 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/30 text-[14px] font-semibold" />
          </div>

          <!-- 规则内容 -->
          <div class="flex flex-col gap-2">
            <label class="text-[11px] font-black uppercase tracking-[0.15em] text-zinc-400 dark:text-zinc-500 px-1">规则内容 (System Prompt Prepend)</label>
            <textarea v-model="editingRule.content" rows="8" placeholder="请在此输入你希望拼接在 system prompt 最前面的内容，大模型会以此作为顶级上下文..."
              class="w-full px-4 py-3 rounded-2xl bg-zinc-50 dark:bg-zinc-800/30 border border-zinc-100 dark:border-zinc-800 focus:outline-none focus:ring-1 focus:ring-blue-500/30 text-[13px] font-medium leading-relaxed resize-none scrollbar-none"></textarea>
          </div>

          <!-- 保存按钮 -->
          <button @click="handleSave" :disabled="!editingRule.name || !editingRule.content"
            class="w-full mt-2 py-3.5 rounded-2xl bg-blue-500 disabled:bg-zinc-200 dark:disabled:bg-zinc-800 text-white disabled:text-zinc-400 dark:disabled:text-zinc-500 text-[14px] font-black shadow-lg shadow-blue-500/10 active:scale-[0.98] transition-all flex items-center justify-center">
            保存规则
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
</style>
