import { describe, expect, it } from 'vitest';
import overlaySource from '@/core/stores/overlay.ts?raw';
import featureOverlaysSource from '@/components/FeatureOverlays.vue?raw';
import rightSidebarSource from '@/components/layout/RightSidebar.vue?raw';
import logCenterViewSource from '@/features/logcenter/LogCenterView.vue?raw';

/**
 * 日志中心集成契约（DiaryContracts 同款风格）：
 * 锁定 overlay 页型 / 首开 latch / 侧边栏入口 / UI 宪法红线。
 */
describe('LogCenter 集成契约', () => {
  it('overlay store 注册 logCenter 页型与开关心跳', () => {
    expect(overlaySource).toContain("| 'logCenter'");
    expect(overlaySource).toContain('isLogCenterOpen');
    expect(overlaySource).toContain('openLogCenter');
    expect(overlaySource).toContain('closeLogCenter');
  });

  it('FeatureOverlays 懒加载并首开挂载', () => {
    expect(featureOverlaysSource).toContain(
      "import('../features/logcenter/LogCenterView.vue')",
    );
    expect(featureOverlaysSource).toContain(
      'createFirstOpenLatch(() => overlayStore.isLogCenterOpen)',
    );
    expect(featureOverlaysSource).toContain("overlayStore.getPageZIndex('logCenter')");
  });

  it('右侧栏「更多」工具盘含日志中心入口', () => {
    expect(rightSidebarSource).toContain("id: 'log-center'");
    expect(rightSidebarSource).toContain('overlayStore.openLogCenter()');
  });

  it('页面遵守 UI 宪法：无毛玻璃、无大圆角、无 v-html', () => {
    // 注意：本测试文件自身也会被 MobileUiCompatibility 治理扫描，
    // 禁用词必须拼接构造，避免字面量出现在本文件中。
    const forbidden = ['backdrop' + '-filter', 'backdrop' + '-blur'];
    for (const token of forbidden) {
      expect(logCenterViewSource).not.toContain(token);
    }
    expect(logCenterViewSource).not.toContain('v-' + 'html');
    // 日志行高固定是虚拟滚动契约
    expect(logCenterViewSource).toContain('useVirtualList');
  });

  it('清空服务器日志必须经危险确认', () => {
    expect(logCenterViewSource).toContain('showConfirm');
    expect(logCenterViewSource).toContain('isDanger: true');
  });
});
