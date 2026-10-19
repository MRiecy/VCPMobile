import { SETTINGS_FIELDS } from '../../../core/config/generated/metadata';
import type { SettingsFieldPath, SettingsFieldValue } from '../../../core/config/fields';
export { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
export type { VoiceInputEncoding, VoiceTtsMode, VoiceVadSensitivity } from '../../../core/config/generated/config';

export const settingDescriptor = (path: SettingsFieldPath) => {
  const [domain, ...parts] = path.split('.');
  const field = SETTINGS_FIELDS.find(item => item.domain === domain && item.path === parts.join('.'));
  if (!field) throw new Error(`Missing configuration descriptor: ${path}`);
  return field;
};
export const settingOptions = <P extends SettingsFieldPath>(path: P, labels: Record<string, string>) =>
  (settingDescriptor(path).options ?? []).map(value => ({ value: value as SettingsFieldValue<P>, label: labels[value] ?? value }));

export const MIMO_TTS_MODES = [
  { value: 'builtin', label: '预置音色', model: 'mimo-v2.5-tts' },
  { value: 'clone', label: '样本复刻', model: 'mimo-v2.5-tts-voiceclone' },
] as const;

// IDs are the official API values, including the four Chinese voice IDs.
const voiceLabels: Record<string, string> = {
  mimo_default: 'MiMo 默认', '冰糖': '冰糖 · 中文女声', '茉莉': '茉莉 · 中文女声',
  '苏打': '苏打 · 中文男声', '白桦': '白桦 · 中文男声', Mia: 'Mia · 英文女声',
  Chloe: 'Chloe · 英文女声', Milo: 'Milo · 英文男声', Dean: 'Dean · 英文男声',
};
export const MIMO_VOICES = settingOptions('voice.tts.voice', voiceLabels);
