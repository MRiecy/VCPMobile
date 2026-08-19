//! nameHue.ts — 名字 → 稳定色相/头像配色的共享工具。
//!
//! 论坛署名头像、Agent 头像等"无图实体"都用同一套散列，保证同一名字
//! 在任何模块颜色一致。仅产出颜色，不涉 DOM。

/** 名字 → 稳定 HSL 色相（0-359）。 */
export function nameHue(name: string): number {
  let hash = 0;
  for (let i = 0; i < name.length; i += 1) {
    hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  }
  return hash % 360;
}

/** 名字 → 头像背景（135° 双色调渐变，辨识度高于单色）。 */
export function nameAvatarBackground(name: string): string {
  const hue = nameHue(name || '?');
  return `linear-gradient(135deg, hsl(${hue} 58% 52%), hsl(${(hue + 36) % 360} 55% 42%))`;
}

/** 名字 → 头像首字符（取第一个非空白字符，大写化拉丁字母）。 */
export function nameInitial(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return '?';
  const first = Array.from(trimmed)[0] ?? '?';
  return first.toUpperCase();
}
