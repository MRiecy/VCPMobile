<script setup lang="ts">
import { ref, onUnmounted, watch } from "vue";
import type { AppSettings } from "../../../core/stores/settings";
import type { SettingsSaveResult } from "../editing/useSettingsEditSession";
import { isIOS } from "../../../core/utils/platform";
import { cancelFileOperation } from "../../../core/utils/nativeFilePicker";
import SettingsActionWithStatus from "../../../components/settings/SettingsActionWithStatus.vue";
import VcpConfirm from "../../../components/ui/VcpConfirm.vue";
import {
  exportConfigBackup,
  pickConfigBackup,
  type ParsedConfigBackup,
} from "../useConfigBackup";

const props = defineProps<{
  active: boolean;
  disabled?: boolean;
  commit: () => Promise<SettingsSaveResult>;
  applyImport: (patch: Partial<AppSettings>) => Promise<void>;
}>();

type Status = { type: "success" | "error" | "loading" | null; message: string };

const exportStatus = ref<Status>({ type: null, message: "" });
const importStatus = ref<Status>({ type: null, message: "" });

let disposed = false;
let importOperationId: string | null = null;

// ------------------------------------------------------------------
// 导出
// ------------------------------------------------------------------
const onExport = async () => {
  if (disposed || !props.active || props.disabled || importOperationId || importing.value || showImportConfirm.value || exportStatus.value.type === "loading") return;
  exportStatus.value = { type: "loading", message: "正在生成备份文件..." };
  try {
    const saved = await props.commit();
    if (disposed || !props.active) return;
    if (!saved.ok) throw new Error(saved.error);
    const result = await exportConfigBackup(saved.snapshot);
    if (disposed || !props.active) return;
    exportStatus.value = {
      type: "success",
      message: `${isIOS ? '已保存到所选文件位置' : '已导出到 下载/VCPMobile'}：${result.displayName}\n文件包含明文密钥，请妥善保管`,
    };
  } catch (e: any) {
    if (disposed) return;
    if (String(e).toLowerCase().includes("cancel")) {
      exportStatus.value = { type: null, message: "" };
      return;
    }
    exportStatus.value = { type: "error", message: `导出失败: ${e}` };
  }
};

// ------------------------------------------------------------------
// 导入
// ------------------------------------------------------------------
const pendingImport = ref<ParsedConfigBackup | null>(null);
const showImportConfirm = ref(false);
const importing = ref(false);

const triggerImport = async () => {
  if (disposed || !props.active || props.disabled || exportStatus.value.type === "loading" || importOperationId || importing.value || showImportConfirm.value) return;
  const operationId = `config_${crypto.randomUUID()}`;
  importOperationId = operationId;
  importStatus.value = { type: "loading", message: "正在选取并解析备份文件..." };
  try {
    const parsed = await pickConfigBackup(operationId);
    if (disposed || importOperationId !== operationId) return;
    pendingImport.value = parsed;
    showImportConfirm.value = true;
    importStatus.value = { type: null, message: "" };
  } catch (e: any) {
    if (disposed || importOperationId !== operationId) return;
    if (String(e).toLowerCase().includes("cancel")) {
      importStatus.value = { type: null, message: "" };
      return;
    }
    importStatus.value = { type: "error", message: `导入失败: ${e?.message ?? e}` };
  } finally {
    if (importOperationId === operationId) importOperationId = null;
  }
};

watch(showImportConfirm, (open) => {
  if (!open) pendingImport.value = null;
});

const deactivate = () => {
  disposed = true;
  showImportConfirm.value = false;
  pendingImport.value = null;
  if (importOperationId) {
    const operationId = importOperationId;
    importOperationId = null;
    void cancelFileOperation(operationId).catch(console.warn);
  }
};
watch(() => props.active, (active) => { if (!active) deactivate(); }, { flush: 'sync' });
onUnmounted(deactivate);

const confirmImport = async () => {
  if (disposed || !props.active || props.disabled || importing.value) return;
  const backup = pendingImport.value;
  pendingImport.value = null;
  showImportConfirm.value = false;
  if (!backup) return;

  importing.value = true;
  importStatus.value = { type: "loading", message: "正在应用配置..." };
  try {
    await props.applyImport(backup.settings);
    if (disposed || !props.active) return;
    importStatus.value = { type: "success", message: "配置导入成功" };
  } catch (e: any) {
    if (disposed || !props.active) return;
    importStatus.value = { type: "error", message: `导入失败: ${e}` };
  } finally {
    if (!disposed) importing.value = false;
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <SettingsActionWithStatus
      title="导出配置"
      :description="isIOS ? '用户身份与应用连接配置，保存到文件' : '用户身份与应用连接配置，保存到系统下载目录'"
      button-variant="primary"
      button-label="导出"
      :button-loading="exportStatus.type === 'loading'"
      :button-disabled="disabled || !!importOperationId || importing || showImportConfirm"
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
      :button-disabled="disabled || exportStatus.type === 'loading'"
      :status-type="importStatus.type"
      :status-message="importStatus.message"
      status-multiline
      @action-click="triggerImport"
    />

    <p class="text-[10px] opacity-40 px-1 italic">
      * 备份文件为明文 JSON（.vcpcfg 后缀），包含 API Key 与密码，请勿分享给他人
    </p>

  </div>

  <VcpConfirm
    v-if="active"
    v-model:is-open="showImportConfirm"
    title="导入配置备份"
    :message="`将覆盖「用户身份」与「应用连接」的现有配置。\n\n备份导出时间：${pendingImport?.exportedAt || '未知'}\n\n确定继续吗？`"
    is-danger
    @confirm="confirmImport"
  />
</template>
