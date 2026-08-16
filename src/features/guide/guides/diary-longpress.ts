/**
 * diary-longpress — 长按日记项教学
 *
 * 触发（D2 用户确认）：日记中心打开 + 可见日记 ≥1 条。
 * 日记列表为虚拟列表（useVirtualList）：锚点挂在虚拟行 button 上，
 * 注册表多元素、步骤解析取首个可见行。
 */
import { defineGuide } from '../registry';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useDiaryStore } from '../../diary/diaryStore';

defineGuide({
  id: 'diary-longpress',
  title: '长按日记项',
  description: '长按进入多选模式，批量移动 / 删除',
  trigger: {
    predicates: [
      {
        name: 'diary-center-open',
        check: () => useOverlayStore().isDiaryCenterOpen,
      },
      {
        name: 'displayed-notes-ge-1',
        check: () => useDiaryStore().displayedNotes.length >= 1,
      },
    ],
  },
  steps: [
    {
      target: 'diary-note-row',
      title: '长按日记项',
      content: '在任意日记条目上长按，进入多选模式。',
      placement: 'bottom',
      demo: 'press-hold',
      // 首次打开日记中心：目录加载 + 虚拟列表行挂载 + 页面滑入动画
      // 可能超过默认 3s，放宽到 6s；行未就绪前继续轮询而非越过。
      waitFor: () => useDiaryStore().displayedNotes.length >= 1,
      waitTimeoutMs: 6000,
    },
    {
      target: 'diary-note-row',
      title: '真实效果',
      content: '进入多选后，底部出现「移动 / 删除」，可批量整理日记。',
      waitFor: () => useDiaryStore().displayedNotes.length >= 1,
      waitTimeoutMs: 6000,
    },
  ],
});
