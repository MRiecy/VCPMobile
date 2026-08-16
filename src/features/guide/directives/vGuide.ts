/**
 * v-guide 指令：将元素注册为目标注册表条目。
 *
 * 用法：`v-guide="'chat-theme-button'"`；支持 v-for / 虚拟列表中
 * 同 id 多元素（注册表按挂载顺序保存，步骤解析取首个有效元素）。
 */
import type { Directive, DirectiveBinding } from 'vue';
import { registerTarget, unregisterTarget } from '../registry';

export const vGuide: Directive<HTMLElement, string> = {
  mounted(el: HTMLElement, binding: DirectiveBinding<string>) {
    if (typeof binding.value !== 'string' || binding.value.length === 0) {
      console.warn('v-guide requires a non-empty string value');
      return;
    }
    registerTarget(binding.value, el);
  },
  updated(el: HTMLElement, binding: DirectiveBinding<string>) {
    if (binding.value === binding.oldValue) return;
    if (typeof binding.oldValue === 'string' && binding.oldValue.length > 0) {
      unregisterTarget(binding.oldValue, el);
    }
    if (typeof binding.value === 'string' && binding.value.length > 0) {
      registerTarget(binding.value, el);
    }
  },
  unmounted(el: HTMLElement, binding: DirectiveBinding<string>) {
    if (typeof binding.value === 'string' && binding.value.length > 0) {
      unregisterTarget(binding.value, el);
    }
  },
};
