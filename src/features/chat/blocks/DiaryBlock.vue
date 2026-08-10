<script setup lang="ts">
import { computed } from "vue";
import type { ContentBlock, MarkdownNode } from "../../../core/types/chat";
import { renderMarkdownNodes } from "../../../core/utils/astRenderer";

const props = defineProps<{
  block: ContentBlock;
  messageId: string;
}>();

const maidName = computed(() => props.block.maid?.trim() || "");
const valetName = computed(() => props.block.valet?.trim() || "");

// 协议约定：Maid 优先；仅 Maid 为空时才采用 Valet，两者都空则保持 Maid 视觉。
const isMaid = computed(() => Boolean(maidName.value) || !valetName.value);
const agentName = computed(() => isMaid.value ? maidName.value : valetName.value);
const agentLabel = computed(() => isMaid.value ? "Maid" : "Valet");
const defaultTitle = computed(() => isMaid.value ? "Maid's Diary" : "Valet's Diary");
const diaryTitle = computed(() => props.block.file_name?.trim() || defaultTitle.value);

function hasNodes(nodes?: MarkdownNode[]): nodes is MarkdownNode[] {
  return Boolean(nodes && nodes.length > 0);
}

function updateCacheKey(side: "target" | "replace"): string | undefined {
  if (props.block.hash === undefined || props.block.hash === null) return undefined;
  return `${String(props.block.hash)}:${side}`;
}

const diaryHtml = computed(() => {
  if (!hasNodes(props.block.nodes)) return "";
  return renderMarkdownNodes(props.block.nodes, props.messageId, props.block.hash);
});

const targetHtml = computed(() => {
  if (!hasNodes(props.block.target_nodes)) return "";
  return renderMarkdownNodes(
    props.block.target_nodes,
    props.messageId,
    updateCacheKey("target"),
  );
});

const replaceHtml = computed(() => {
  if (!hasNodes(props.block.replace_nodes)) return "";
  return renderMarkdownNodes(
    props.block.replace_nodes,
    props.messageId,
    updateCacheKey("replace"),
  );
});
</script>

<template>
  <div
    v-if="block.type === 'diary'"
    class="vcp-diary-block"
    :class="{ 'is-valet': !isMaid }"
    data-vcp-block-type="diary"
    data-vcp-preserve-children="true"
  >
    <div class="vcp-diary-header">
      <span class="vcp-diary-title">{{ diaryTitle }}</span>
      <span v-if="block.date" class="vcp-diary-date">{{ block.date }}</span>
    </div>

    <div v-if="agentName || block.folder" class="vcp-diary-maid-info">
      <template v-if="agentName">
        <span class="vcp-diary-agent-label">{{ agentLabel }}:</span>
        <span class="vcp-diary-maid-name">{{ agentName }}</span>
      </template>
      <span v-if="agentName && block.folder" class="vcp-diary-meta-separator">·</span>
      <template v-if="block.folder">
        <span class="vcp-diary-folder-label">Folder:</span>
        <span class="vcp-diary-folder-name">{{ block.folder }}</span>
      </template>
    </div>

    <div
      v-if="hasNodes(block.nodes)"
      class="vcp-diary-content vcp-markdown-block"
      v-html="diaryHtml"
    />
    <div v-else class="vcp-diary-content vcp-markdown-block">
      <p>{{ block.content || "[日记内容解析失败]" }}</p>
    </div>
  </div>

  <div
    v-else-if="block.type === 'diary-update'"
    class="vcp-diary-update-block"
    :class="{ 'is-valet': !isMaid }"
    data-vcp-block-type="diary-update"
    data-vcp-preserve-children="true"
  >
    <div class="vcp-diary-update-header">
      <span class="vcp-diary-update-title">DailyNote Update</span>
      <span v-if="agentName || block.folder" class="vcp-diary-update-meta">
        <span v-if="agentName" class="vcp-diary-maid-name">{{ agentName }}</span>
        <span v-if="agentName && block.folder" class="vcp-diary-meta-separator">·</span>
        <span v-if="block.folder" class="vcp-diary-folder-name">{{ block.folder }}</span>
      </span>
    </div>

    <div class="vcp-diary-update-body">
      <div class="vcp-diary-update-side vcp-diary-update-before">
        <div class="vcp-diary-update-label">A</div>
        <div
          v-if="block.target?.trim() && hasNodes(block.target_nodes)"
          class="vcp-diary-update-content vcp-markdown-block"
          v-html="targetHtml"
        />
        <div v-else-if="block.target?.trim()" class="vcp-diary-update-content vcp-markdown-block">
          <p>{{ block.target }}</p>
        </div>
        <div v-else class="vcp-diary-update-content"><em>原文解析失败</em></div>
      </div>

      <div class="vcp-diary-update-arrow" aria-hidden="true">→</div>

      <div class="vcp-diary-update-side vcp-diary-update-after">
        <div class="vcp-diary-update-label">B</div>
        <div
          v-if="block.replace?.trim() && hasNodes(block.replace_nodes)"
          class="vcp-diary-update-content vcp-markdown-block"
          v-html="replaceHtml"
        />
        <div v-else-if="block.replace?.trim()" class="vcp-diary-update-content vcp-markdown-block">
          <p>{{ block.replace }}</p>
        </div>
        <div v-else class="vcp-diary-update-content"><em>替换内容解析失败</em></div>
      </div>
    </div>
  </div>
</template>
