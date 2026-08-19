<script setup lang="ts">
import { ref, watch } from "vue";
import { storeToRefs } from "pinia";
import {
  ChevronRight,
  Copy,
  FolderInput,
  PackageCheck,
} from "lucide-vue-next";
import RefreshButton from "../../../components/ui/RefreshButton.vue";
import { useVcpCliStore } from "../vcpCliStore";

const store = useVcpCliStore();
const {
  skills,
  skillsLoading,
  skillsError,
  selectedSkillId,
  selectedSkill,
  selectedSkillContent,
  skillLoading,
  skillError,
  skillCatalog,
  skillImportCandidate,
  skillImportBusy,
  skillImportError,
} = storeToRefs(store);

const copyFeedback = ref("");

async function copySkill(): Promise<void> {
  if (!selectedSkillContent.value) return;
  copyFeedback.value = "";
  try {
    await navigator.clipboard.writeText(selectedSkillContent.value);
    copyFeedback.value = "SKILL.md 已复制";
  } catch (error) {
    copyFeedback.value = `复制失败：${error instanceof Error ? error.message : String(error)}`;
  }
}

watch(selectedSkillId, () => {
  copyFeedback.value = "";
});
</script>

<template>
  <section
    v-if="selectedSkillId"
    class="flex min-h-0 flex-1 flex-col"
    aria-label="Skill 正文"
    data-vcp-cli-role="skill-detail"
  >
    <div
      class="shrink-0 border-b border-black/10 px-3 py-2 dark:border-white/10"
    >
      <div
        class="flex items-start gap-3 border-l-2 border-[var(--highlight-text)] pl-3"
      >
        <div class="min-w-0 flex-1">
          <h2 class="truncate text-[12px] font-bold">
            {{ selectedSkill?.name || selectedSkillId }}
          </h2>
          <p class="mt-1 font-mono text-[8px] opacity-45">
            READ ONLY · SKILL.md · 不执行脚本
          </p>
        </div>
        <button
          type="button"
          class="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-black/10 px-2.5 text-[10px] font-bold disabled:opacity-35 dark:border-white/10"
          data-vcp-cli-action="materialize-skill"
          :disabled="skillLoading"
          @click="store.materializeSelectedSkill"
        >
          <PackageCheck :size="13" />物化
        </button>
        <button
          type="button"
          class="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-black/10 px-2.5 text-[10px] font-bold disabled:opacity-35 dark:border-white/10"
          data-vcp-cli-action="copy-skill"
          :disabled="!selectedSkillContent"
          @click="copySkill"
        >
          <Copy :size="13" />复制
        </button>
      </div>
      <dl
        v-if="selectedSkill"
        class="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 font-mono text-[8px]"
      >
        <dt class="opacity-35">RESOURCE</dt>
        <dd class="min-w-0 truncate opacity-60">
          {{ selectedSkill.skill_root }}/{{ selectedSkill.resource_path }}
        </dd>
        <dt class="opacity-35">SHA256</dt>
        <dd class="min-w-0 break-all opacity-60">{{ selectedSkill.sha256 }}</dd>
        <template v-if="selectedSkill.materialized_path">
          <dt class="opacity-35">WORKSPACE</dt>
          <dd class="min-w-0 break-all text-[var(--highlight-text)]">
            {{ selectedSkill.materialized_path }}
          </dd>
        </template>
      </dl>
      <p v-if="copyFeedback" class="mt-2 text-[9px] opacity-60" role="status">
        {{ copyFeedback }}
      </p>
      <p
        v-if="skillError"
        class="mt-2 whitespace-pre-wrap font-mono text-[9px] leading-4 text-red-500"
        role="status"
      >
        {{ skillError.code }} · {{ skillError.message }}
      </p>
    </div>

    <div
      v-if="skillLoading"
      class="flex min-h-0 flex-1 items-center justify-center text-[10px] opacity-45"
      role="status"
    >
      正在读取受控 Skill catalog…
    </div>
    <pre
      v-else
      class="vcp-scrollable no-swipe min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words bg-black/[0.025] px-3 py-3 font-mono text-[10px] leading-[1.6] select-text dark:bg-white/[0.025]"
      data-vcp-cli-role="skill-content"
      >{{ selectedSkillContent || "(SKILL.md 暂无正文)" }}</pre
    >

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pb-[calc(var(--vcp-safe-bottom,48px)+8px)] pt-2 text-[9px] leading-4 dark:border-white/10"
    >
      <span
        v-if="selectedSkill?.truncated"
        class="border-l-2 border-amber-500 pl-2 font-bold text-amber-600 dark:text-amber-400"
        >正文超过读取预算，当前内容已截断。</span
      >
      <span v-else class="opacity-45">
        阅读不会创建 Job，也不会授予 Skill 中脚本任何 Android 权限。
      </span>
    </footer>
  </section>

  <section v-else class="flex min-h-0 flex-1 flex-col" aria-label="Skills">
    <div
      class="flex min-h-12 shrink-0 items-center border-b border-black/10 px-3 dark:border-white/10"
    >
      <div class="min-w-0 flex-1">
        <h2 class="text-[11px] font-bold">受控 Skill catalog</h2>
        <p class="mt-0.5 text-[9px] opacity-45">
          canonical 不挂载；导入先审阅，脚本永不自动执行。
        </p>
      </div>
      <button
        type="button"
        class="inline-flex h-9 shrink-0 items-center gap-1.5 rounded-lg border border-black/10 px-2.5 text-[9px] font-bold disabled:opacity-35 dark:border-white/10"
        data-vcp-cli-action="import-skill"
        :disabled="skillImportBusy"
        @click="store.inspectSkillImport"
      >
        <FolderInput :size="13" />导入 ZIP
      </button>
      <RefreshButton
        bare
        class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg opacity-45 active:opacity-100"
        label="刷新 Skills"
        :size="14"
        :loading="skillsLoading"
        :disabled="skillsLoading"
        @refresh="store.loadSkills(true)"
      />
    </div>

    <div
      v-if="skillImportCandidate"
      class="shrink-0 border-b border-black/10 border-l-2 border-l-[var(--highlight-text)] px-3 py-2 dark:border-white/10"
      data-vcp-cli-role="skill-import-review"
    >
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <p class="truncate text-[10px] font-bold">
            {{ skillImportCandidate.name }}
          </p>
          <p class="mt-1 font-mono text-[8px] opacity-50">
            {{ skillImportCandidate.skill_id }} ·
            <span v-if="skillImportCandidate.version"
              >v{{ skillImportCandidate.version }} ·</span
            >
            {{ skillImportCandidate.resource_count }} FILES ·
            {{ skillImportCandidate.total_bytes }} B
          </p>
          <p class="mt-1 text-[9px] leading-4 opacity-65">
            {{ skillImportCandidate.description }}
          </p>
          <p class="mt-0.5 truncate font-mono text-[8px] opacity-35">
            SOURCE {{ skillImportCandidate.source_name }}
          </p>
          <p class="mt-0.5 break-all font-mono text-[8px] opacity-35">
            TREE {{ skillImportCandidate.tree_sha256 }}
          </p>
          <p
            v-if="skillImportCandidate.replaces_existing"
            class="mt-1 border-l-2 border-amber-500 pl-2 text-[9px] text-amber-600 dark:text-amber-400"
          >
            将替换同 ID 的已导入版本；内置 Skill 不可替换。
          </p>
          <p
            v-for="warning in skillImportCandidate.warnings"
            :key="warning"
            class="mt-1 text-[9px] leading-4 text-amber-600 dark:text-amber-400"
          >
            {{ warning }}
          </p>
        </div>
        <div class="flex shrink-0 flex-col gap-1.5">
          <button
            type="button"
            class="min-h-8 rounded-md bg-[var(--highlight-text)] px-2 text-[9px] font-bold text-white disabled:opacity-35"
            :disabled="skillImportBusy"
            data-vcp-cli-action="commit-skill-import"
            @click="store.commitSkillImport"
          >
            确认导入
          </button>
          <button
            type="button"
            class="min-h-8 rounded-md border border-black/10 px-2 text-[9px] disabled:opacity-35 dark:border-white/10"
            :disabled="skillImportBusy"
            data-vcp-cli-action="discard-skill-import"
            @click="store.discardSkillImport"
          >
            放弃
          </button>
        </div>
      </div>
    </div>

    <div
      v-if="skillCatalog?.warnings.length || skillImportError"
      class="shrink-0 border-b border-amber-500/20 px-3 py-2 text-[9px] leading-4"
      role="status"
    >
      <p v-if="skillImportError" class="font-mono text-red-500">
        {{ skillImportError.code }} · {{ skillImportError.message }}
      </p>
      <p
        v-for="warning in skillCatalog?.warnings || []"
        :key="warning"
        class="text-amber-600 dark:text-amber-400"
      >
        {{ warning }}
      </p>
    </div>

    <p
      v-if="skillsError"
      class="shrink-0 border-b border-red-500/20 px-3 py-2 font-mono text-[9px] leading-4 text-red-500"
      role="status"
    >
      {{ skillsError.code }} · {{ skillsError.message }}
    </p>
    <div
      v-if="skillsLoading && skills.length === 0"
      class="flex min-h-0 flex-1 items-center justify-center text-[10px] opacity-45"
      role="status"
    >
      正在列出 Skills…
    </div>
    <div
      v-else-if="skills.length === 0"
      class="flex min-h-0 flex-1 items-center justify-center px-6 text-center text-[10px] leading-5 opacity-40"
    >
      当前没有可读取且完整性校验通过的 Skill。
    </div>
    <div
      v-else
      class="min-h-0 flex-1 overflow-y-auto divide-y divide-black/10 no-rubber-band dark:divide-white/10"
    >
      <button
        v-for="skill in skills"
        :key="skill.id"
        type="button"
        class="flex min-h-15 w-full items-center gap-3 border-l-2 border-transparent px-3 py-2 text-left active:border-[var(--highlight-text)] active:bg-black/[0.035] dark:active:bg-white/[0.035]"
        data-vcp-cli-role="skill-row"
        @click="store.openSkill(skill)"
      >
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[11px] font-semibold">{{
            skill.name
          }}</span>
          <span
            class="mt-1 flex min-w-0 flex-wrap gap-x-2 gap-y-0.5 font-mono text-[8px] opacity-45"
          >
            <span>{{ skill.id }}</span>
            <span v-if="skill.version">v{{ skill.version }}</span>
            <span>{{ skill.source }}</span>
          </span>
          <span class="mt-0.5 block truncate font-mono text-[8px] opacity-30">
            SHA256 {{ skill.sha256 }}
          </span>
        </span>
        <ChevronRight :size="14" class="shrink-0 opacity-25" />
      </button>
    </div>

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pb-[calc(var(--vcp-safe-bottom,48px)+8px)] pt-2 text-[9px] leading-4 opacity-45 dark:border-white/10"
    >
      ZIP 仅经原生 picker 导入。确认后保存 canonical
      对象；执行仍需先物化，再另发 run。
    </footer>
  </section>
</template>
