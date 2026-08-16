/**
 * plus-longpress — 聊天框「+」按钮教学（单击附件 + 长按上下文注入）
 *
 * 触发（D2 用户确认的推演）：输入框解锁（当前话题存在）+ 左右栏均关闭。
 * 真机反馈轮起：单击与长按均唤起真实业务——
 * 单击 = 真实展开附件面板；长按 = 真实弹出 VCP 上下文注入规则抽屉。
 */
import { defineGuide } from '../registry';
import { attachMenuOpen, setAttachMenu } from '../../chat/attachMenuController';
import { useTarvenStore } from '../../../core/stores/tarvenStore';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useLayoutStore } from '../../../core/stores/layout';
import { useOverlayStore } from '../../../core/stores/overlay';

defineGuide({
  id: 'plus-longpress',
  title: '长按聊天框 + 按钮',
  description: '单击附件菜单，长按上下文注入管理器',
  prepare: () => {
    // 回放时清空页面栈并关闭两侧抽屉，确保聊天框 + 按钮不被遮挡。
    useOverlayStore().popToRoot();
    const layoutStore = useLayoutStore();
    layoutStore.setLeftDrawer(false);
    layoutStore.setRightDrawer(false);
  },
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
      title: '单击展开附件',
      content: '单击 + 按钮，展开附件面板。',
      placement: 'top',
      demo: 'tap',
      // 用户点「下一步」时执行：真实展开附件面板。
      perform: () => setAttachMenu(true),
      undo: () => setAttachMenu(false),
    },
    {
      target: 'chat-attach-menu',
      title: '附件面板',
      content: '三种附件入口：拍摄、相册、文件。',
      placement: 'top',
      // 面板展开有过渡动画，等待就绪再聚光。
      waitFor: () => attachMenuOpen.value,
    },
    {
      target: 'chat-plus-button',
      title: '长按上下文注入',
      content: '长按 + 约 0.6 秒，打开 VCP 上下文注入规则仓。',
      placement: 'top',
      demo: 'press-hold',
      // 用户点「下一步」时执行：收起附件面板，弹出真实规则仓抽屉。
      perform: () => {
        setAttachMenu(false);
        useTarvenStore().isSelectorOpen = true;
      },
      undo: () => {
        setAttachMenu(false);
        useTarvenStore().isSelectorOpen = false;
      },
    },
    {
      target: 'tarven-selector',
      title: '规则仓',
      content: '在此启用 / 停用注入规则；有规则启用时 + 按钮会显示绿色小点。',
      placement: 'top',
      waitFor: () => useTarvenStore().isSelectorOpen,
      waitTimeoutMs: 6000,
    },
  ],
});
