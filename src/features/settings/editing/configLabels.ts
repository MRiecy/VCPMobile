import type { ConfigDomain, FieldIssue } from '../../../core/config/generated/config';
import type { SettingsFieldPath } from '../../../core/config/fields';

export const CONFIG_DOMAIN_LABELS: Record<ConfigDomain, string> = {
  identity: '用户身份', server: '主服务器', log: '日志连接', sync: '数据同步',
  distributed: '分布式', conversation: '对话辅助', voice: '语音', navigation: '侧栏排序',
  jev: 'JEV',
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
  'jev.enabled': '启用 JEV', 'jev.provider': '服务提供商',
  'jev.typesafe.apiUrl': 'TypeSafe API 地址', 'jev.typesafe.apiKey': 'TypeSafe API Key', 'jev.typesafe.model': 'TypeSafe 模型',
  'jev.openrouter.apiUrl': 'OpenRouter API 地址', 'jev.openrouter.apiKey': 'OpenRouter API Key', 'jev.openrouter.model': 'OpenRouter 模型',
  'jev.timeoutMs': '响应超时', 'jev.maxRetries': '最大重试次数', 'jev.retryBaseDelayMs': '重试等待时间',
  'jev.proxyUrl': '代理地址', 'jev.httpReferer': '应用网址', 'jev.appTitle': '应用名称',
};

export function configIssueLabel(issue: FieldIssue): string {
  const field = FIELD_LABELS[`${issue.domain}.${issue.field}` as SettingsFieldPath];
  return field ? `${CONFIG_DOMAIN_LABELS[issue.domain]} · ${field}` : CONFIG_DOMAIN_LABELS[issue.domain];
}
