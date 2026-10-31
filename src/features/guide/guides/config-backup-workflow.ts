import { defineGuide } from "../registry";
import { getActivePinia } from "pinia";
import { useSettingsStore } from "../../../core/stores/settings";
import { useAssistantStore } from "../../../core/stores/assistant";

export const configBackupWorkflowGuide = defineGuide({
  id: "config-backup-workflow",
  title: "配置全量备份与两阶段恢复",
  description: "学习如何通过配置备份快速初始化连接，并在同步后恢复助手独立音色与群聊设置。",
  trigger: {
    predicates: [
      {
        name: "hasServerUrl",
        check: () => {
          if (!getActivePinia()) return false;
          return Boolean(useSettingsStore().settings?.server?.url);
        },
      },
      {
        name: "noAssistantsYet",
        check: () => {
          if (!getActivePinia()) return false;
          const store = useAssistantStore();
          return store.agents.length === 0 && store.groups.length === 0;
        },
      },
    ],
    settleMs: 1000,
  },
  steps: [
    {
      target: "settings-config-backup-section",
      title: "第一步：恢复全局配置",
      content: "点击「恢复配置」导入 .vcpcfg.json，恢复服务器地址、主题与语音偏好。",
      placement: "bottom",
      demo: "tap",
    },
    {
      target: "sidebar-sync-button",
      title: "第二步：执行数据同步",
      content: "在数据同步面板中拉取电脑端 VChat 的智能体、群聊与聊天记录。",
      placement: "right",
      demo: "tap",
    },
    {
      target: null,
      title: "第三步：还原助手专属设置",
      content: "同步完成后，系统将自动弹窗。点击一键补全即可复原移动端专用提示词与群组模式！",
      placement: "bottom",
    },
  ],
});

export default configBackupWorkflowGuide;
