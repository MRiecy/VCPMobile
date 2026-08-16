import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia, getActivePinia } from 'pinia';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import { defineGuide, registerTarget, unregisterTarget } from '@/features/guide/registry';
import { useGuideStore } from '@/features/guide/stores/guideStore';
import GuideOverlay from '@/features/guide/components/GuideOverlay.vue';

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve('1.1.4')),
}));

const MOCK_RECT = {
  top: 120,
  left: 80,
  width: 200,
  height: 60,
  right: 280,
  bottom: 180,
  x: 80,
  y: 120,
  toJSON: () => ({}),
};

// 固定几何：happy-dom 默认 rect 全为 0，覆盖为确定性坐标。
Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return MOCK_RECT as DOMRect;
};
Element.prototype.scrollIntoView = function scrollIntoView() {};

function createTarget(id: string): HTMLElement {
  const el = document.createElement('div');
  document.body.appendChild(el);
  registerTarget(id, el);
  return el;
}

beforeEach(() => {
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.useRealTimers();
  const store = useGuideStore();
  store.pendingQueue = [];
  while (store.activeGuideId) {
    store.finish();
  }
  unregisterTarget('overlay-target', document.querySelector('[data-target="overlay"]') as HTMLElement);
  document.body.innerHTML = '';
  const pinia = getActivePinia() as unknown as { _e?: { stop: () => void } } | null;
  pinia?._e?.stop();
});

describe('GuideOverlay', () => {
  it('shows veil → spot + card + demo, advances by button, exits on 我知道了', async () => {
    vi.useFakeTimers();
    defineGuide({
      id: 'ov-walk',
      title: 'walk',
      description: 'walk',
      steps: [
        {
          target: 'overlay-target',
          title: '第一步',
          content: '内容一',
          placement: 'bottom',
          demo: 'press-hold',
        },
        { target: 'overlay-target', title: '第二步', content: '内容二' },
      ],
    });
    const store = useGuideStore();
    const target = createTarget('overlay-target');
    target.setAttribute('data-target', 'overlay');

    const wrapper = mount(GuideOverlay);
    store.start('ov-walk');
    await nextTick();

    // 目标未稳定：兜底暗纱在场
    expect(wrapper.find('.guide-overlay').exists()).toBe(true);
    expect(wrapper.find('.guide-veil').exists()).toBe(true);

    // 稳定性采样：连续两次采样一致后聚光出现（veil 挖洞层 + frame 描边层）
    vi.advanceTimersByTime(140);
    await nextTick();
    expect(wrapper.find('.guide-spot').exists()).toBe(true);
    expect(wrapper.find('.guide-spot-veil').exists()).toBe(true);
    expect(wrapper.find('.guide-spot-frame').exists()).toBe(true);
    // 兜底暗纱让位（淡出过渡 + 帧调度完成后移除）
    vi.advanceTimersByTime(500);
    await nextTick();
    expect(wrapper.find('.guide-veil').exists()).toBe(false);

    // 卡片：[1/2] 计数 + 标题 + 正文 + 下一步按钮
    const card = wrapper.find('.guide-card');
    expect(card.exists()).toBe(true);
    expect(card.text()).toContain('[1/2]');
    expect(card.text()).toContain('第一步');
    expect(card.text()).toContain('内容一');
    expect(wrapper.find('.guide-btn').text()).toBe('下一步');

    // press-hold 演示动画挂载（进度环 + 手势图标）
    expect(wrapper.find('.guide-demo').exists()).toBe(true);
    expect(wrapper.find('.demo-ring-progress').exists()).toBe(true);
    expect(wrapper.find('.demo-icon-glyph').exists()).toBe(true);

    // 推进到第二步：纯说明步骤无演示动画
    await wrapper.find('.guide-btn').trigger('click');
    await nextTick();
    expect(card.text()).toContain('[2/2]');
    expect(card.text()).toContain('内容二');
    expect(wrapper.find('.guide-btn').text()).toBe('我知道了');
    expect(wrapper.find('.guide-demo').exists()).toBe(false);

    // 末步收尾：覆盖层关闭 + 完成态写入
    await wrapper.find('.guide-btn').trigger('click');
    await nextTick();
    expect(wrapper.find('.guide-overlay').exists()).toBe(false);
    expect(store.isCompleted('ov-walk')).toBe(true);

    wrapper.unmount();
  });

  it('silently skips an unresolvable step after the wait timeout', async () => {
    vi.useFakeTimers();
    defineGuide({
      id: 'ov-skip',
      title: 'skip',
      description: 'skip',
      steps: [
        { target: 'never-mounted', title: '缺失', content: '将被越过', waitTimeoutMs: 500 },
        { target: 'overlay-target', title: '到达', content: '第二步可见' },
      ],
    });
    const store = useGuideStore();
    const target = createTarget('overlay-target');
    target.setAttribute('data-target', 'overlay');

    const wrapper = mount(GuideOverlay);
    store.start('ov-skip');
    await nextTick();

    // 目标缺失期间：暗纱在场但卡片按钮始终可点（异常退出保障）
    expect(wrapper.find('.guide-veil').exists()).toBe(true);
    expect(wrapper.find('.guide-btn').text()).toBe('下一步');

    // 超时后静默越过第一步
    vi.advanceTimersByTime(520);
    await nextTick();
    expect(wrapper.find('.guide-card').text()).toContain('到达');
    expect(wrapper.find('.guide-card').text()).toContain('[2/2]');

    wrapper.unmount();
  });

  it('finishes and records completion when the whole guide is unresolvable', async () => {
    vi.useFakeTimers();
    defineGuide({
      id: 'ov-dead',
      title: 'dead',
      description: 'dead',
      steps: [{ target: 'never-mounted', title: '缺失', content: '无目标', waitTimeoutMs: 300 }],
    });
    const store = useGuideStore();

    const wrapper = mount(GuideOverlay);
    store.start('ov-dead');
    await nextTick();
    expect(wrapper.find('.guide-overlay').exists()).toBe(true);

    vi.advanceTimersByTime(320);
    await nextTick();
    expect(wrapper.find('.guide-overlay').exists()).toBe(false);
    expect(store.isCompleted('ov-dead')).toBe(true);

    wrapper.unmount();
  });

  it('runs step perform on entry and runs the undo stack in reverse on finish', async () => {
    vi.useFakeTimers();
    const performed: string[] = [];
    const undone: string[] = [];
    defineGuide({
      id: 'ov-real',
      title: 'real',
      description: 'real',
      steps: [
        {
          target: 'overlay-target',
          title: '第一步',
          content: '内容一',
          perform: () => performed.push('s1'),
          undo: () => undone.push('s1'),
        },
        {
          target: 'overlay-target',
          title: '第二步',
          content: '内容二',
          perform: () => performed.push('s2'),
          undo: () => undone.push('s2'),
        },
      ],
    });
    const store = useGuideStore();
    const target = createTarget('overlay-target');
    target.setAttribute('data-target', 'overlay');

    const wrapper = mount(GuideOverlay);
    store.start('ov-real');
    await nextTick();
    vi.advanceTimersByTime(140);
    await nextTick();
    // 步骤进入时执行一次 perform（稳定性重采样不重复执行）
    expect(performed).toEqual(['s1']);

    await wrapper.find('.guide-btn').trigger('click');
    await nextTick();
    expect(performed).toEqual(['s1', 's2']);
    // 步间推进不执行 undo（真实业务状态跨步保留，由下一步 perform 自理）
    expect(undone).toEqual([]);

    vi.advanceTimersByTime(140);
    await nextTick();
    await wrapper.find('.guide-btn').trigger('click'); // 我知道了 → 整场结束
    await nextTick();
    expect(undone).toEqual(['s2', 's1']); // 逆序清理

    wrapper.unmount();
  });

  it('keeps the spotlight mounted (no veil flash) while advancing steps', async () => {
    vi.useFakeTimers();
    defineGuide({
      id: 'ov-glide',
      title: 'glide',
      description: 'glide',
      steps: [
        { target: 'overlay-target', title: '第一步', content: '内容一' },
        { target: 'other-target', title: '第二步', content: '内容二' },
      ],
    });
    const store = useGuideStore();
    createTarget('overlay-target').setAttribute('data-target', 'overlay');
    createTarget('other-target').setAttribute('data-target', 'overlay');

    const wrapper = mount(GuideOverlay);
    store.start('ov-glide');
    await nextTick();
    vi.advanceTimersByTime(140);
    await nextTick();
    expect(wrapper.find('.guide-spot').exists()).toBe(true);
    // 首步兜底暗纱完成淡出让位
    vi.advanceTimersByTime(500);
    await nextTick();

    // 推进步骤：新目标未稳定期间旧聚光保留，不重现全屏暗纱（防闪回归）
    await wrapper.find('.guide-btn').trigger('click');
    await nextTick();
    expect(wrapper.find('.guide-veil').exists()).toBe(false);
    expect(wrapper.find('.guide-spot').exists()).toBe(true);

    wrapper.unmount();
  });
});
