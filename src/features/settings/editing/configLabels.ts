import type { ConfigDomain, FieldIssue } from '../../../core/config/generated/config';
import type { SettingsFieldPath } from '../../../core/config/fields';
import { SETTINGS_FIELDS } from '../../../core/config/generated/metadata';

export const CONFIG_DOMAIN_LABELS: Record<ConfigDomain, string> = {
  identity: '用户身份', server: '主服务器', log: '日志连接', sync: '数据同步',
  distributed: '分布式', conversation: '对话辅助', voice: '语音', navigation: '侧栏排序',
};

const FIELD_LABELS: Record<SettingsFieldPath, string> = {
  'identity.userName': '用户名',
  'server.url': 'Chat 服务器地址', 'server.apiKey': 'Chat API Key', 'server.endpointMode': 'Chat 请求入口',
  'server.adminUsername': '管理员账号', 'server.adminPassword': '管理员密码', 'server.fileKey': '图床密钥',
  'log.url': '日志服务器地址', 'log.key': '日志鉴权 Key',
  'sync.httpUrl': '同步 HTTP 地址', 'sync.serverUrl': '同步 WebSocket 地址', 'sync.token': '同步 Token',
  'sync.logLevel': '同步日志级别', 'sync.prerenderEnabled': '同步预渲染',
  'distributed.enabled': '分布式连接开关', 'distributed.wsUrl': '分布式服务器地址',
  'distributed.vcpKey': '分布式鉴权 Key', 'distributed.deviceName': '设备名称',
  'conversation.topicSummaryModel': '话题总结模型',
  'voice.connection.apiBaseUrl': '语音 API 地址', 'voice.connection.apiKey': '语音 API Key',
  'voice.asr.stream': '流式识别结果', 'voice.asr.inputEncoding': '语音输入编码', 'voice.asr.language': '识别语言',
  'voice.tts.stream': '流式朗读', 'voice.tts.voice': '朗读音色', 'voice.tts.mode': '音色模式',
  'voice.tts.stylePrompt': '默认演绎指导', 'voice.tts.cloneSampleId': '复刻参考音频',
  'voice.guidance.audioTagRules': '音频标签生成规则', 'voice.guidance.autoGuidance': '模型自动生成指导',
  'voice.guidance.guidanceRules': '自然语言指导生成规则',
  'voice.vad.sensitivity': '人声检测灵敏度', 'voice.vad.endSilenceMs': '说完等待时间',
  'voice.vad.playbackOnsetMs': '朗读时插话确认',
  'navigation.agentOrder': '智能体排序', 'navigation.groupOrder': '群组排序',
};

export function configIssueLabel(issue: FieldIssue): string {
  const field = FIELD_LABELS[`${issue.domain}.${issue.field}` as SettingsFieldPath];
  return field ? `${CONFIG_DOMAIN_LABELS[issue.domain]} · ${field}` : CONFIG_DOMAIN_LABELS[issue.domain];
}

/** Credential-coupled fields are reset together, following Rust's validation groups. */
export function configResetFields(path: SettingsFieldPath): SettingsFieldPath[] {
  const field = SETTINGS_FIELDS.find(item => `${item.domain}.${item.path}` === path);
  if (!field || !['connection', 'adminCredentials'].includes(field.validationGroup)) return [path];
  return SETTINGS_FIELDS.filter(item => item.domain === field.domain && item.validationGroup === field.validationGroup)
    .map(item => `${item.domain}.${item.path}` as SettingsFieldPath);
}

export function configResetLabel(path: SettingsFieldPath): string {
  const fields = configResetFields(path);
  if (fields.length === 1) return '使用默认值';
  const domain = path.split('.')[0] as ConfigDomain;
  return fields.includes('server.adminUsername') ? '恢复管理员凭据默认值' : `恢复${CONFIG_DOMAIN_LABELS[domain]}${domain === 'log' ? '' : '连接'}默认值`;
}

export function configResetDescription(path: SettingsFieldPath): string {
  return configResetFields(path).map(field => FIELD_LABELS[field]).join('、');
}
