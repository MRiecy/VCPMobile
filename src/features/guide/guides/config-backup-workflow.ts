import { defineGuide, hasTarget } from "../registry";

export const configBackupWorkflowGuide = defineGuide({
  id: "config-backup-workflow",
  title: "配置备份与换机恢复",
  description: "首次进入服务器连接页时，指导如何通过 .vcpcfg 文件快速恢复配置，并在同步后复原专属人设与群聊模式。",
  trigger: {
    predicates: [
      {
        name: "connection-page-mounted",
        check: () => hasTarget("settings-config-import-action"),
      },
    ],
    settleMs: 600,
  },
  steps: [
    {
      target: "settings-config-import-action",
      title: "阶段一：导入配置备份",
      content:
        "若您已有 .vcpcfg.json 配置备份，可点击此处「导入」。\n\n" +
        "系统将快速恢复：\n" +
        "• 服务器连接地址、API 密钥与身份设置\n" +
        "• 语音服务、主题排版与置顶偏好\n" +
        "• 常用模型收藏与 Tarven 提示词注入规则\n\n" +
        "*若无备份文件，直接在上方填写服务器信息即可。智能体专属人设此时会自动暂存，待同步后生效。*",
      placement: "top",
    },
    {
      target: "settings-config-import-action",
      title: "阶段二：执行数据同步",
      content:
        "完成基础配置后，需在主界面右侧工具面板中执行「数据同步」。\n\n" +
        "同步子系统将拉取：\n" +
        "• 智能体角色列表与群组成员关系\n" +
        "• 完整历史聊天记录与多媒体附件\n" +
        "• 角色头像二进制数据",
      placement: "top",
    },
    {
      target: "settings-config-import-action",
      title: "阶段三：恢复助手专属设置",
      content:
        "数据同步完成后，系统将自动检测暂存的本地偏好，并弹出「发现待生效的实体配置」确认窗口。\n\n" +
        "点击确定即可一键恢复：\n" +
        "• 智能体移动端专属系统提示词与发音音色\n" +
        "• 模型采样温度发送控制开关\n" +
        "• 群聊运行模式（顺序发言、自然发言、JEV 领航等）与上下文消息窗口",
      placement: "top",
    },
  ],
});

export default configBackupWorkflowGuide;
