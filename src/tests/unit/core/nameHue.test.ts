import { describe, expect, it } from 'vitest';
import { nameAvatarBackground, nameHue, nameInitial } from '@/core/utils/nameHue';

describe('nameHue 共享散列', () => {
  it('同名稳定、不同名大概率不同色', () => {
    expect(nameHue('小娜')).toBe(nameHue('小娜'));
    expect(nameHue('小娜')).not.toBe(nameHue('小雨'));
    expect(nameHue('')).toBe(0);
  });

  it('头像背景为 135° 双色渐变，含色相', () => {
    const bg = nameAvatarBackground('小娜');
    expect(bg).toContain('linear-gradient(135deg');
    expect(bg).toContain(`hsl(${nameHue('小娜')}`);
  });

  it('首字符：空白回退 ?，拉丁字母大写，中文原样', () => {
    expect(nameInitial('小娜')).toBe('小');
    expect(nameInitial('alice')).toBe('A');
    expect(nameInitial('  ')).toBe('?');
    expect(nameInitial('')).toBe('?');
  });
});
