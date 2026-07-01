import { mount, type MountingOptions } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { Component, Plugin } from 'vue';

export function mountWithPinia(component: Component, options: MountingOptions<Record<string, unknown>> = {}) {
  const pinia = createPinia();
  setActivePinia(pinia);

  return mount(component, {
    ...options,
    global: {
      ...(options.global ?? {}),
      plugins: [pinia, ...((options.global?.plugins as Plugin[]) ?? [])],
    },
  });
}
