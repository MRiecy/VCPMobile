import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import ChatAvatar from '../../../features/chat/components/ChatAvatar.vue';
import InputEnhancer from '../../../features/chat/InputEnhancer.vue';
import NarrationWave from '../../../features/chat/components/NarrationWave.vue';
import { useBubbleNarration } from '../../../features/chat/voice/useBubbleNarration';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { mockInvoke } from '../../mocks/tauri';

let pinia: Pinia;
const wrappers: VueWrapper[] = [];

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  mockInvoke('start_tts_session', () => ({ configured: true }));
  mockInvoke('submit_tts_text', () => undefined);
  mockInvoke('stop_tts', () => undefined);
});

afterEach(async () => {
  wrappers.splice(0).forEach(w => w.unmount());
  disposePinia(pinia);
  await flushPromises();
  vi.restoreAllMocks();
});

describe('Bubble Narration UI Enhancements', () => {
  describe('ChatAvatar narration waveform indicator', () => {
    it('activates avatar wave indicator when isNarrating is true for assistant avatar', () => {
      const wrapper = mount(ChatAvatar, {
        props: {
          fallbackText: 'Assistant',
          isUser: false,
          isNarrating: true,
        },
        global: {
          plugins: [pinia],
          stubs: { VcpAvatar: true },
        },
      });
      wrappers.push(wrapper);

      expect(wrapper.classes()).toContain('is-narrating');
      expect(wrapper.attributes('data-narrating')).toBe('true');
      expect(wrapper.find('.vcp-message-avatar-indicator').exists()).toBe(true);
    });

    it('does not show wave indicator when isNarrating is false', () => {
      const wrapper = mount(ChatAvatar, {
        props: {
          fallbackText: 'Assistant',
          isUser: false,
          isNarrating: false,
        },
        global: {
          plugins: [pinia],
          stubs: { VcpAvatar: true },
        },
      });
      wrappers.push(wrapper);

      expect(wrapper.classes()).not.toContain('is-narrating');
      expect(wrapper.attributes('data-narrating')).toBe('false');
    });

    it('does not display wave indicator for user avatar even if isNarrating is passed', () => {
      const wrapper = mount(ChatAvatar, {
        props: {
          fallbackText: 'User',
          isUser: true,
          isNarrating: true,
        },
        global: {
          plugins: [pinia],
          stubs: { VcpAvatar: true },
        },
      });
      wrappers.push(wrapper);

      expect(wrapper.classes()).not.toContain('is-narrating');
      expect(wrapper.find('.vcp-message-avatar-indicator').exists()).toBe(false);
    });
  });

  describe('InputEnhancer primary action button transformation and narration wave', () => {
    function setupComposer() {
      const session = useChatSessionStore();
      session.setConversation({ id: 'agent-1', type: 'agent', name: '助手' }, 'topic-1');
      const history = useChatHistoryStore();
      history.loadedConversationKey = session.currentConversationKey;
      history.currentChatHistory = [{ id: 'msg-1', role: 'assistant', content: '测试正文', timestamp: 1, blocks: [] }];

      const wrapper = mount(InputEnhancer, {
        global: {
          plugins: [pinia],
          directives: { longpress: {}, guide: {} },
          stubs: {
            Transition: false,
            TransitionGroup: true,
            GroupStopAllButton: true,
            GroupInviteBar: true,
            StagedAttachmentPreview: true,
            VcpAvatar: true,
            TarvenSelector: true,
          },
        },
      });
      wrappers.push(wrapper);
      return { wrapper, session, history };
    }

    it('displays voice call button when idle and switches to stop narration button during bubble playback', async () => {
      const { wrapper, history } = setupComposer();
      const narration = useBubbleNarration();

      const primaryBtn = wrapper.find('button.primary-action-btn');
      expect(primaryBtn.exists()).toBe(true);
      expect(primaryBtn.classes()).toContain('is-voice');
      expect(primaryBtn.attributes('aria-label')).toBe('开始语音通话');
      expect(wrapper.findComponent(NarrationWave).exists()).toBe(false);

      // 启动消息朗读
      const actionKey = history.captureMessageActionKey('msg-1')!;
      await narration.startBubbleNarration(actionKey, '朗读测试文本');
      await flushPromises();

      // 朗读期间：主按钮变成红色停止按钮，并显示 NarrationWave
      expect(primaryBtn.classes()).toContain('is-stop');
      expect(primaryBtn.classes()).not.toContain('is-voice');
      expect(primaryBtn.attributes('aria-label')).toBe('停止朗读');
      expect(wrapper.findComponent(NarrationWave).exists()).toBe(true);

      // 点击主按钮停止朗读
      const stopSpy = vi.spyOn(narration, 'stopBubbleNarration');
      await primaryBtn.trigger('click');
      await flushPromises();

      expect(stopSpy).toHaveBeenCalled();
      // 停止朗读后恢复为通话按钮
      expect(primaryBtn.classes()).toContain('is-voice');
      expect(primaryBtn.classes()).not.toContain('is-stop');
      expect(primaryBtn.attributes('aria-label')).toBe('开始语音通话');
      expect(wrapper.findComponent(NarrationWave).exists()).toBe(false);
    });

    it('switches to send button when user types text while narrating', async () => {
      const { wrapper, history } = setupComposer();
      const narration = useBubbleNarration();

      const actionKey = history.captureMessageActionKey('msg-1')!;
      await narration.startBubbleNarration(actionKey, '朗读测试文本');
      await flushPromises();

      const textarea = wrapper.find('textarea.vcp-textarea');
      await textarea.setValue('新输入的内容');
      await flushPromises();

      const primaryBtn = wrapper.find('button.primary-action-btn');
      expect(primaryBtn.classes()).toContain('is-send');
      expect(primaryBtn.classes()).not.toContain('is-stop');
      expect(primaryBtn.classes()).not.toContain('is-voice');
    });
  });

  describe('NarrationWave component', () => {
    it('mounts canvas element and cleans up animation frame on unmount', () => {
      const wrapper = mount(NarrationWave, {
        props: { active: true },
      });
      wrappers.push(wrapper);

      const canvas = wrapper.find('canvas');
      expect(canvas.exists()).toBe(true);
      expect(wrapper.find('.vcp-narration-wave').exists()).toBe(true);
    });
  });
});
