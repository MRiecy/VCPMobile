<script setup lang="ts">
import { computed, ref, onUnmounted, watch } from "vue";
import type { SettingsSaveResult } from "../editing/useSettingsEditSession";
import { isIOS } from "../../../core/utils/platform";
import { cancelFileOperation } from "../../../core/utils/nativeFilePicker";
import SettingsActionWithStatus from "../../../components/settings/SettingsActionWithStatus.vue";
import VcpConfirm from "../../../components/ui/VcpConfirm.vue";
import {
  exportConfigBackup,
  captureBackupDomains,
  captureBackupGroupPreferences,
  pickConfigBackup,
  configBackupScopes,
  unavailableBackupTheme,
  type BackupImportResult,
  type ParsedConfigBackup,
} from "../useConfigBackup";

const props = defineProps<{
  active: boolean;
  disabled?: boolean;
  commit: () => Promise<SettingsSaveResult>;
  applyImport: (backup: ParsedConfigBackup) => Promise<BackupImportResult>;
}>();

type Status = { type: "success" | "error" | "loading" | "info" | null; message: string };

const exportStatus = ref<Status>({ type: null, message: "" });
const importStatus = ref<Status>({ type: null, message: "" });

let disposed = false;
let importOperationId: string | null = null;
let exportOperationId: string | null = null;

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
    const [domains, groupPreferences] = await Promise.all([captureBackupDomains(), captureBackupGroupPreferences()]);
    if (disposed || !props.active) return;
    exportOperationId = `config_export_${crypto.randomUUID()}`;
    const result = await exportConfigBackup(domains, exportOperationId, groupPreferences);
    if (disposed || !props.active) return;
    exportStatus.value = {
      type: "success",
      message: `${isIOS ? '已保存到配置备份目录' : '已导出到 下载/VCPMobile'}：${result.displayName}\n文件包含明文密钥，请妥善保管`,
    };
  } catch (e: any) {
    if (disposed) return;
    if (String(e).toLowerCase().includes("cancel")) {
      exportStatus.value = { type: null, message: "" };
      return;
    }
    exportStatus.value = { type: "error", message: `导出失败: ${e}` };
  } finally {
    exportOperationId = null;
  }
};

// ------------------------------------------------------------------
// 导入
// ------------------------------------------------------------------
const pendingImport = ref<ParsedConfigBackup | null>(null);
const showImportConfirm = ref(false);
const importing = ref(false);
const importConfirmation = computed(() => {
  const backup = pendingImport.value;
  if (!backup) return '';
  const missing = unavailableBackupTheme(backup);
  return `将覆盖备份中包含的配置：${configBackupScopes(backup).join('、')}。未包含的设置保持当前值。`
    + (Object.keys(backup.groupPreferences ?? {}).length ? '\n\n群聊本机设置按字段恢复，缺项保留当前值；尚无本机配置时，缺项使用默认值。已删除的群组不会恢复。' : '')
    + (missing === null ? '' : `\n\n主题 ${missing} 在当前版本不可用，将保留当前主题与深浅模式。`)
    + `\n\n备份导出时间：${backup.exportedAt || '未知'}\n\n确定继续吗？`;
});

const triggerImport = async () => {
  if (disposed || !props.active || props.disabled || exportStatus.value.type === "loading" || importOperationId || importing.value || showImportConfirm.value) return;
  const operationId = `config_${crypto.randomUUID()}`;
  importOperationId = operationId;
  importStatus.value = { type: "loading", message: "正在选取并解析备份文件..." };
  try {
    const parsed = await pickConfigBackup(operationId);
    if (disposed || importOperationId !== operationId) return;
    if (!configBackupScopes(parsed).length) throw new Error('备份未包含可恢复的配置');
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
  if (exportOperationId) {
    const operationId = exportOperationId;
    exportOperationId = null;
    void cancelFileOperation(operationId).catch(console.warn);
  }
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
    const result = await props.applyImport(backup);
    if (disposed || !props.active) return;
    const summary = result.failed.length
      ? (result.applied.length ? '配置部分导入，部分项目保存失败' : '配置导入未完成')
      : result.skipped.length ? '配置导入完成，部分项目已跳过' : '配置导入成功';
    importStatus.value = {
      type: result.failed.length ? 'error' : result.skipped.length ? 'info' : 'success',
      message: [
        summary,
        ...(result.applied.length ? [`已恢复：${result.applied.join('、')}`] : []),
        ...result.skipped.map(message => `已跳过：${message}`),
        ...result.failed.map(failure => `未恢复：${failure.scope}：${failure.message}`),
      ].join('\n'),
    };
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
      description="备份身份、连接、语音、JEV、群聊本机设置、主题与聊天偏好"
      button-variant="primary"
      button-label="导出"
      :button-loading="exportStatus.type === 'loading'"
      :button-disabled="disabled || !!importOperationId || importing || showImportConfirm"
      :status-type="exportStatus.type"
      :status-message="exportStatus.message"
      status-multiline
      @action-click="onExport"
    />

    <p v-if="isIOS" class="text-xs text-[var(--secondary-text)] px-1 leading-relaxed">
      首次导出时选择保存目录；后续导入与导出会优先使用该位置。
    </p>

    <div class="border-t border-black/5 dark:border-white/5 pt-2"></div>

    <SettingsActionWithStatus
      title="导入配置"
      description="恢复 .vcpcfg.json 配置备份，未包含的选项保持当前值"
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
      * 备份文件为明文 JSON，包含 API Key 与密码，请勿分享给他人
    </p>
    <p class="text-xs text-[var(--secondary-text)] px-1 leading-relaxed">
      复刻音频文件不包含在备份中；更换设备后需重新导入参考音频。
    </p>
    <p class="text-xs text-[var(--secondary-text)] px-1 leading-relaxed">
      群聊本机设置按群组 ID、按字段恢复，缺项保留；新手机缺项使用默认值，尚未同步的群组会等待绑定。
    </p>

  </div>

  <VcpConfirm
    v-if="active"
    v-model:is-open="showImportConfirm"
    title="导入配置备份"
    :message="importConfirmation"
    is-danger
    @confirm="confirmImport"
  />
</template>
