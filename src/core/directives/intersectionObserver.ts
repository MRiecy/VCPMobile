// intersectionObserver.ts
import type { Directive } from 'vue';

export const vIntersectionObserver: Directive = {
  mounted(el, binding) {
    const options = binding.value || { threshold: 0.1 };
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          el.dispatchEvent(new CustomEvent('intersect', { detail: entry }));
          if (binding.modifiers.once) {
            observer.unobserve(el);
          }
        } else {
          el.dispatchEvent(new CustomEvent('unintersect', { detail: entry }));
        }
      });
    }, options);

    el._observer = observer;
    observer.observe(el);
  },
  unmounted(el) {
    if (el._observer) {
      el._observer.disconnect();
      delete el._observer;
    }
  },
};
