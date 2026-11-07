import { beforeEach, describe, expect, it, vi } from 'vitest';
import PfHistoryTab from '../../../../features/projectforge/components/PfHistoryTab.vue';
import PfReportTab from '../../../../features/projectforge/components/PfReportTab.vue';
import {
  createMockStore,
  flush,
  makeDetail,
  makeHistoryRow,
  makeProject,
  mountWithPinia,
} from './testUtils';

const state = vi.hoisted(() => ({ store: {} as any }));

// happy-dom 下 DOMPurify 的真实行为与 WebView 不一致（会丢标签），
// 单测用受控 sanitize 替身：仅剥离 script，用于验证组件确实调用了清洗与渲染流程。
const sanitizeSpy = vi.hoisted(() =>
  vi.fn((html: string) => String(html).replace(/<script[\s\S]*?<\/script>/gi, '')),
);

vi.mock('dompurify', () => ({ default: { sanitize: sanitizeSpy } }));

vi.mock('../../../../features/projectforge/projectForgeStore', () => ({
  useProjectForgeStore: () => state.store,
}));

function mountTab(component: any, overrides: Record<string, any> = {}) {
  state.store = createMockStore(overrides);
  return mountWithPinia(component);
}

describe('PfHistoryTab — 搜索历史', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('初始展示引导文案，提交过滤表单后调用 searchHistory 并渲染结果表', async () => {
    const searchHistory = vi.fn().mockResolvedValue([makeHistoryRow()]);
    const { wrapper } = mountTab(PfHistoryTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory,
    });
    await flush();

    expect(wrapper.text()).toContain('输入条件后搜索本工程的变动历史。');

    await wrapper.find('form.pf-filter').trigger('submit');
    await flush();

    expect(searchHistory).toHaveBeenCalledWith({ limit: 100 }, { projectId: 'proj1' });
    expect(wrapper.text()).toContain('共 1 条');
    expect(wrapper.text()).toContain('n21');
    expect(wrapper.text()).toContain('b5');
    expect(wrapper.text()).toContain('edit');
    expect(wrapper.text()).toContain('tools/video.py');
    expect(wrapper.text()).toContain('@claude');
    expect(wrapper.text()).toContain('+7');
    expect(wrapper.text()).toContain('-2');
    expect(wrapper.text()).toContain('补充说明');
  });

  it('全部过滤条件（keyword / content / file / byMaid / op / limit）按契约透传', async () => {
    const searchHistory = vi.fn().mockResolvedValue([]);
    const { wrapper } = mountTab(PfHistoryTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory,
    });
    await flush();

    const inputs = wrapper.findAll('.pf-filter input');
    await inputs[0].setValue('format-tool');
    await inputs[1].setValue('subprocess.run');
    await inputs[2].setValue('tools/*.py');
    await inputs[3].setValue('claude');
    await wrapper.findAll('.pf-filter select')[0].setValue('edit');
    await wrapper.findAll('.pf-filter select')[1].setValue('300');
    await wrapper.find('form.pf-filter').trigger('submit');
    await flush();

    expect(searchHistory).toHaveBeenCalledWith(
      {
        limit: 300,
        keyword: 'format-tool',
        content: 'subprocess.run',
        file: 'tools/*.py',
        byMaid: 'claude',
        op: 'edit',
      },
      { projectId: 'proj1' },
    );
  });

  it('点结果行 emit open-diff({ kind: node })', async () => {
    const { wrapper } = mountTab(PfHistoryTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory: vi.fn().mockResolvedValue([makeHistoryRow({ id: 44 })]),
    });
    await flush();

    await wrapper.find('form.pf-filter').trigger('submit');
    await flush();

    await wrapper.find('.pf-history-row').trigger('click');
    expect(wrapper.emitted('open-diff')?.[0]).toMatchObject([{ kind: 'node', node: { id: 44 } }]);
  });

  it('外部跳转 initialFilter（byMaid）自动填入并执行搜索', async () => {
    const searchHistory = vi.fn().mockResolvedValue([makeHistoryRow()]);
    state.store = createMockStore({
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory,
    });
    const { wrapper } = mountWithPinia(PfHistoryTab, {
      props: { initialFilter: { byMaid: 'claude', _stamp: 1 } },
    });
    await flush();

    expect(searchHistory).toHaveBeenCalledWith(expect.objectContaining({ byMaid: 'claude' }), {
      projectId: 'proj1',
    });
    const byMaidInput = wrapper.findAll('.pf-filter input')[3].element as HTMLInputElement;
    expect(byMaidInput.value).toBe('claude');
  });

  it('空结果与搜索失败展示空态 / 错误态', async () => {
    const empty = mountTab(PfHistoryTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory: vi.fn().mockResolvedValue([]),
    });
    await flush();
    await empty.wrapper.find('form.pf-filter').trigger('submit');
    await flush();
    expect(empty.wrapper.text()).toContain('没有匹配的记录。');
    empty.wrapper.unmount();

    const failed = mountTab(PfHistoryTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      searchHistory: vi.fn().mockRejectedValue(new Error('桥接请求超时')),
    });
    await flush();
    await failed.wrapper.find('form.pf-filter').trigger('submit');
    await flush();
    expect(failed.wrapper.text()).toContain('桥接请求超时');
    expect(failed.wrapper.text()).toContain('重试');
  });
});

describe('PfReportTab — 验收报告', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染报告 Markdown（DOMPurify 清洗 script）与提交者 / 状态', async () => {
    const detail = makeDetail({
      project: makeProject({
        report: '# 验收报告\n\n**结论**：通过。\n\n<script>window.__evil = 1</script>',
        report_by: 'claude',
        status: 'review',
      }),
    });
    const { wrapper } = mountTab(PfReportTab, { detail });
    await flush();

    expect(wrapper.find('.pf-markdown').exists()).toBe(true);
    const html = wrapper.find('.pf-markdown').html();
    expect(html).toContain('<h1');
    expect(html).toContain('验收报告');
    expect(html).not.toContain('<script');
    expect(sanitizeSpy).toHaveBeenCalled();
    expect(wrapper.text()).toContain('提交者');
    expect(wrapper.text()).toContain('@claude');
    expect(wrapper.text()).toContain('待验收');
  });

  it('没有报告时展示「尚未提交验收报告。」', async () => {
    const { wrapper } = mountTab(PfReportTab, { detail: makeDetail() });
    await flush();
    expect(wrapper.text()).toContain('尚未提交验收报告。');
  });

  it('验收结论必填：空结论不进入确认、不调用 submitReport', async () => {
    const { wrapper } = mountTab(PfReportTab, { detail: makeDetail() });
    await flush();

    await wrapper.find('.pf-report-actions .pf-btn').trigger('click');
    await wrapper.find('form.pf-report-form').trigger('submit');
    await flush();

    expect(wrapper.text()).toContain('验收结论（conclusion）为必填项');
    expect(wrapper.text()).not.toContain('确认提交验收报告');
    expect(state.store.submitReport).not.toHaveBeenCalled();
  });

  it('提交报告：二次确认后调用 submitReport 并 emit submitted', async () => {
    const { wrapper } = mountTab(PfReportTab, {
      detail: makeDetail(),
      currentProjectId: 'proj1',
      submitReport: vi.fn().mockResolvedValue(undefined),
    });
    await flush();

    await wrapper.find('.pf-report-actions .pf-btn').trigger('click');
    await wrapper.find('form.pf-report-form textarea').setValue('功能全部完成，验收通过');
    await wrapper.findAll('form.pf-report-form textarea')[1].setValue('遗留：真机夜间模式待测');
    await wrapper.find('form.pf-report-form select').setValue('accepted');
    await wrapper.find('form.pf-report-form').trigger('submit');
    await flush();

    expect(wrapper.text()).toContain('确认提交验收报告');
    expect(wrapper.text()).toContain('accepted（已验收）');

    await wrapper.find('.pf-confirm-actions .pf-btn.primary').trigger('click');
    await flush();

    expect(state.store.submitReport).toHaveBeenCalledWith({
      conclusion: '功能全部完成，验收通过',
      issues: '遗留：真机夜间模式待测',
      status: 'accepted',
    });
    expect(wrapper.emitted('submitted')).toBeTruthy();
  });

  it('取消二次确认则不提交', async () => {
    const { wrapper } = mountTab(PfReportTab, { detail: makeDetail() });
    await flush();

    await wrapper.find('.pf-report-actions .pf-btn').trigger('click');
    await wrapper.find('form.pf-report-form textarea').setValue('结论内容');
    await wrapper.find('form.pf-report-form').trigger('submit');
    await flush();

    const cancelButton = wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text() === '取消')!;
    await cancelButton.trigger('click');
    await flush();

    expect(state.store.submitReport).not.toHaveBeenCalled();
  });
});
