<script setup lang="ts">
import { ref } from "vue";
import type { AppSettings } from "../../../core/stores/settings";
import { useSettingsStore } from "../../../core/stores/settings";
import SettingsActionWithStatus from "../../../components/settings/SettingsActionWithStatus.vue";
import VcpConfirm from "../../../components/ui/VcpConfirm.vue";
import {
  exportConfigBackup,
  readConfigBackupFile,
  type ParsedConfigBackup,
} from "../useConfigBackup";

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save-request"): void;
  (e: "config-imported"): void;
}>();

type Status = { type: "success" | "error" | "loading" | null; message: string };

const exportStatus = ref<Status>({ type: null, message: "" });
const importStatus = ref<Status>({ type: null, message: "" });

const settingsStore = useSettingsStore();

// ------------------------------------------------------------------
// 导出
// ------------------------------------------------------------------
const onExport = async () => {
  // 先把页面上未保存的修改落盘，保证导出内容与用户所见一致
  emit("save-request");

  exportStatus.value = { type: "loading", message: "正在生成备份文件..." };
  try {
    const result = await exportConfigBackup(props.settings);
    exportStatus.value = {
      type: "success",
      message: `已导出到 下载/VCPMobile/${result.displayName}\n文件包含明文密钥，请妥善保管`,
    };
  } catch (e: any) {
    exportStatus.value = { type: "error", message: `导出失败: ${e}` };
  }
};

// ------------------------------------------------------------------
// 导入
// ------------------------------------------------------------------
const fileInput = ref<HTMLInputElement | null>(null);
const pendingImport = ref<ParsedConfigBackup | null>(null);
const showImportConfirm = ref(false);
const importing = ref(false);

const triggerImport = () => {
  fileInput.value?.click();
};

const onFileChange = async (e: Event) => {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = ""; // 允许重复选择同一文件
  if (!file) return;

  importStatus.value = { type: "loading", message: "正在解析备份文件..." };
  try {
    pendingImport.value = await readConfigBackupFile(file);
    showImportConfirm.value = true;
    importStatus.value = { type: null, message: "" };
  } catch (e: any) {
    importStatus.value = { type: "error", message: `导入失败: ${e?.message ?? e}` };
  }
};

const confirmImport = async () => {
  const backup = pendingImport.value;
  pendingImport.value = null;
  if (!backup) return;

  importing.value = true;
  importStatus.value = { type: "loading", message: "正在应用配置..." };
  try {
    await settingsStore.updateSettings(backup.settings);
    emit("config-imported");
    importStatus.value = { type: "success", message: "配置导入成功" };
  } catch (e: any) {
    importStatus.value = { type: "error", message: `导入失败: ${e}` };
  } finally {
    importing.value = false;
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <SettingsActionWithStatus
      title="导出配置"
      description="用户身份、服务器连接与分布式节点配置，保存到系统下载目录"
      button-variant="primary"
      button-label="导出"
      :button-loading="exportStatus.type === 'loading'"
      :status-type="exportStatus.type"
      :status-message="exportStatus.message"
      status-multiline
      @action-click="onExport"
    />

    <div class="border-t border-black/5 dark:border-white/5 pt-2"></div>

    <SettingsActionWithStatus
      title="导入配置"
      description="从 .vcpcfg 备份文件一键恢复全部连接配置"
      button-variant="secondary"
      button-label="导入"
      :button-loading="importStatus.type === 'loading' || importing"
      :status-type="importStatus.type"
      :status-message="importStatus.message"
      status-multiline
      @action-click="triggerImport"
    />

    <p class="text-[10px] opacity-40 px-1 italic">
      * 备份文件为明文 JSON（.vcpcfg 后缀），包含 API Key 与密码，请勿分享给他人
    </p>

    <input
      type="file"
      ref="fileInput"
      class="hidden"
      accept=".vcpcfg,application/json"
      @change="onFileChange"
    />
  </div>

  <VcpConfirm
    v-model:is-open="showImportConfirm"
    title="导入配置备份"
    :message="`将覆盖「用户身份」「服务器连接」与「分布式节点」的现有配置。\n\n备份导出时间：${pendingImport?.exportedAt || '未知'}\n\n确定继续吗？`"
    is-danger
    @confirm="confirmImport"
  />
</template>
