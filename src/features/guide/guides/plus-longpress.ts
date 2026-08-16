/**
 * plus-longpress — 长按聊天框「+」按钮教学
 *
 * 触发（D2 用户确认的推演）：输入框解锁（当前话题存在）+ 左右栏均关闭。
 * 长按 = Tarven 上下文注入规则选择器；单击 = 附件菜单。
 */
import { defineGuide } from '../registry';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useLayoutStore } from '../../../core/stores/layout';
import { useOverlayStore } from '../../../core/stores/overlay';

defineGuide({
  id: 'plus-longpress',
  title: '长按聊天框 + 按钮',
  description: '单击附件菜单，长按上下文注入管理器',
  trigger: {
    predicates: [
      {
        name: 'workspace-not-occluded',
        // 页面栈（设置/日记中心等）盖住聊天主界面时，+ 按钮不可见。
        check: () => useOverlayStore().pageStack.length === 0,
      },
      {
        name: 'input-unlocked',
        check: () => {
          const sessionStore = useChatSessionStore();
          return !!sessionStore.currentTopicId && !!sessionStore.currentSelectedItem;
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
      target: 'chat-plus-button',
      title: '聊天框 + 按钮',
      content: '单击展开附件菜单（相机 / 相册 / 文件）；长按打开 VCP 上下文注入管理器。',
      placement: 'top',
      demo: 'press-hold',
      demoHint: ['上下文注入规则'],
    },
    {
      target: 'chat-plus-button',
      title: '真实效果',
      content: '长按弹出上下文注入规则列表，可为回答启用不同注入规则；有规则启用时按钮会带有绿色小点。',
    },
  ],
});
