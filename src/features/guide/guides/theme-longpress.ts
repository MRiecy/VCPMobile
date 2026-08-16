/**
 * theme-longpress — 长按「亮暗模式」按钮教学
 *
 * 触发（用户第 2 轮亲自定义的前置条件）：已完成 sidebar-gestures
 * + 当前话题已加载 + 非 system 消息 ≥4 + 标题非默认格式 + 左右栏均关闭。
 * 「识别标准同自动话题总结」：chatHistoryStore.summarizeTopic 的默认标题
 * 正则与消息计数标准。
 */
import { defineGuide } from '../registry';
import { openPresentationMenu } from '../../chat/presentationMenu';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useLayoutStore } from '../../../core/stores/layout';
import { useOverlayStore } from '../../../core/stores/overlay';

const DEFAULT_TITLE_RE = /^(新话题|新会话) \d{2}:\d{2}:\d{2}$/;

defineGuide({
  id: 'theme-longpress',
  title: '长按亮暗模式按钮',
  description: '单击切换深浅主题，长按切换消息呈现样式',
  trigger: {
    requires: ['sidebar-gestures'],
    predicates: [
      {
        name: 'workspace-not-occluded',
        // 页面栈（设置/日记中心等）盖住聊天主界面时，主题按钮不可见。
        check: () => useOverlayStore().pageStack.length === 0,
      },
      {
        name: 'topic-loaded',
        check: () => {
          const sessionStore = useChatSessionStore();
          return !!sessionStore.currentTopicId && !!sessionStore.currentSelectedItem;
        },
      },
      {
        name: 'non-system-messages-ge-4',
        check: () =>
          useChatHistoryStore().currentChatHistory.filter((m) => m.role !== 'system')
            .length >= 4,
      },
      {
        name: 'title-not-default',
        check: () => {
          const name = useChatSessionStore().currentTopic?.name ?? '';
          return !DEFAULT_TITLE_RE.test(name);
        },
      },
      {
        name: 'drawers-closed',
        check: () => {
          const layoutStore = useLayoutStore();
          return !layoutStore.leftDrawerOpen && !layoutStore.rightDrawerOpen;
        },
      },
    ],
  },
  steps: [
    {
      target: 'chat-theme-button',
      title: '长按亮暗按钮',
      content: '长按约 0.6 秒，弹出「消息呈现」菜单。',
      placement: 'bottom',
      demo: 'press-hold',
      // 真实业务：弹出真实的「消息呈现」菜单（ContextMenuSheet）。
      perform: openPresentationMenu,
      undo: () => useOverlayStore().closeContextMenu(),
    },
    {
      target: 'context-menu-sheet',
      title: '消息呈现菜单',
      content: '在「气泡 / 统一 / 杂志」中选择一种排版样式。',
      placement: 'top',
      undo: () => useOverlayStore().closeContextMenu(),
    },
  ],
});
