export type LocalPreferenceReadResult =
  | { ok: true; raw: string | null }
  | { ok: false; error: string };

export type LocalPreferenceWriteResult =
  | { ok: true }
  | { ok: false; error: string };

/** 只处理存储访问；key、编码、默认值与修复策略由偏好所属领域决定。 */
export function readLocalPreference(key: string): LocalPreferenceReadResult {
  try {
    return { ok: true, raw: localStorage.getItem(key) };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}

export function writeLocalPreference(key: string, value: string): LocalPreferenceWriteResult {
  try {
    localStorage.setItem(key, value);
    return { ok: true };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}
