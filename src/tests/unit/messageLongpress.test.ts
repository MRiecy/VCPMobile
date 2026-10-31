import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, type VueWrapper } from '@vue/test-utils';
import { defineComponent } from 'vue';
import { vLongpress } from '../../core/directives/longpress';

function dispatchPointer(target: EventTarget, type: string, init: PointerEventInit = {}): PointerEvent {
  const event = new PointerEvent(type, {
    bubbles: true, cancelable: true, pointerId: 7, pointerType: 'touch', isPrimary: true,
    clientX: 20, clientY: 30, button: type === 'contextmenu' ? 2 : 0,
    buttons: type === 'pointerup' || type === 'contextmenu' ? 0 : 1,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

describe('message long press ownership', () => {
  let wrapper: VueWrapper;
  const handler = vi.fn();
  beforeEach(() => {
    vi.useFakeTimers();
    wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      setup: () => ({ handler }),
      template: `<section><div class="unrelated-scroll" style="overflow:auto"></div>
        <div class="message-scroll" style="overflow:auto"><div v-longpress.message="handler">
        <header><span class="name">Agent</span><img class="avatar"><svg class="decoration"><path /></svg></header>
        <div data-message-content class="vcp-markdown-block select-text">
          <p>ordinary <strong>prose</strong></p>
          <div class="ast-render-container"><span class="tail">streamed</span></div>
          <img class="vcp-emoticon"><img class="viewer-image"><svg class="viewer-svg"><path /></svg>
          <a href="https://example.com"><em>link</em></a><button><span>action</span></button>
          <pre><code>selectable code</code></pre>
          <div contenteditable="plaintext-only"><span class="editor">editable</span></div>
          <div data-vcp-button="send">send</div><div class="mermaid-placeholder">graph</div>
          <div onclick="this.textContent='clicked'">custom HTML action</div>
        </div>
        <div data-longpress-native><span class="special">component action</span></div>
        <div data-longpress-native class="attachment"><span>file.pdf</span></div>
      </div></div></section>`,
    }), { attachTo: document.body });
  });
  afterEach(() => {
    wrapper.unmount();
    window.getSelection()?.removeAllRanges();
    vi.useRealTimers();
  });

  it('opens on ordinary rendered text, AST text, headers, avatars and decorative media', () => {
    for (const selector of ['strong', '.tail', '.name', '.avatar', '.decoration path', '.vcp-emoticon']) {
      const target = wrapper.get(selector).element;
      dispatchPointer(target, 'pointerdown');
      vi.advanceTimersByTime(600);
      expect(handler, selector).toHaveBeenCalledTimes(1);
      dispatchPointer(target, 'pointerup');
      handler.mockClear();
    }
  });

  it('routes a touch contextmenu reported on a text node to its enclosing message', () => {
    // Browsers may report a text node rather than its enclosing inline element.
    const inline = wrapper.get('strong').element;
    dispatchPointer(inline, 'pointerdown');
    dispatchPointer(inline.firstChild!, 'contextmenu');
    expect(handler).toHaveBeenCalledTimes(1);
    dispatchPointer(inline, 'pointerup');
  });

  it('leaves independently interactive descendants and code selection untouched', () => {
    for (const selector of ['a em', 'button span', 'pre code', '.editor', '[data-vcp-button]',
      '.mermaid-placeholder', '.viewer-image', '.viewer-svg path', '[onclick]', '.special', '.attachment span']) {
      const target = wrapper.get(selector).element;
      dispatchPointer(target, 'pointerdown');
      vi.advanceTimersByTime(600);
      expect(dispatchPointer(target, 'contextmenu').defaultPrevented, selector).toBe(false);
      const selection = new Event('selectstart', { bubbles: true, cancelable: true });
      target.dispatchEvent(selection);
      expect(selection.defaultPrevented, `${selector} selectstart`).toBe(false);
      dispatchPointer(target, 'pointerup');
    }
    expect(handler).not.toHaveBeenCalled();
  });

  it('prevents ordinary native selection from winning the same held gesture', () => {
    const prose = wrapper.get('p');
    dispatchPointer(prose.element, 'pointerdown');
    const selectionEvent = new Event('selectstart', { bubbles: true, cancelable: true });
    prose.element.dispatchEvent(selectionEvent);
    expect(selectionEvent.defaultPrevented).toBe(true);
    // Even a WebView that selected before delivering selectstart cannot veto the menu.
    const range = document.createRange();
    range.selectNodeContents(prose.element);
    window.getSelection()?.addRange(range);
    vi.advanceTimersByTime(600);
    expect(handler).toHaveBeenCalledTimes(1);
    expect(window.getSelection()?.isCollapsed).toBe(true);
    dispatchPointer(prose.element, 'pointerup');
  });

  it('tolerates small finger jitter and deduplicates the following contextmenu', () => {
    const prose = wrapper.get('p').element;
    dispatchPointer(prose, 'pointerdown');
    dispatchPointer(prose, 'pointermove', { clientX: 26, clientY: 35 });
    vi.advanceTimersByTime(600);
    dispatchPointer(prose, 'contextmenu', { clientX: 26, clientY: 35 });
    expect(handler).toHaveBeenCalledTimes(1);
    dispatchPointer(prose, 'pointerup', { clientX: 26, clientY: 35 });
  });

  it('keeps a stationary message press when a different pane scrolls', () => {
    const prose = wrapper.get('p').element;
    dispatchPointer(prose, 'pointerdown');
    const otherPane = wrapper.get<HTMLElement>('.unrelated-scroll').element;
    otherPane.scrollTop = 40;
    otherPane.dispatchEvent(new Event('scroll'));
    vi.advanceTimersByTime(600);
    expect(handler).toHaveBeenCalledTimes(1);
    dispatchPointer(prose, 'pointerup');
  });

  it('associates a browser contextmenu without the original pointer ID with the active touch only', () => {
    const prose = wrapper.get('p').element;
    dispatchPointer(prose, 'pointerdown', { pointerId: 7 });
    vi.advanceTimersByTime(494);
    // The device trace established touch type, not a stable contextmenu ID.
    // Model the browser's default ID independently of the down/up identity.
    dispatchPointer(prose, 'contextmenu', { pointerId: 0 });
    dispatchPointer(prose, 'contextmenu', { pointerId: 0 });
    expect(handler).toHaveBeenCalledTimes(1);
    dispatchPointer(prose, 'pointerup', { pointerId: 7 });
    vi.advanceTimersByTime(1000);
    dispatchPointer(prose, 'contextmenu', { pointerId: 0 });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it.each(['movement', 'ancestor scroll', 'multitouch', 'pointercancel'])('cancels %s without reopening on a late touch contextmenu', (reason) => {
    const prose = wrapper.get('p').element;
    dispatchPointer(prose, 'pointerdown');
    if (reason === 'movement') dispatchPointer(prose, 'pointermove', { clientY: 45 });
    if (reason === 'ancestor scroll') {
      const messageList = wrapper.get<HTMLElement>('.message-scroll').element;
      messageList.scrollTop = 40;
      messageList.dispatchEvent(new Event('scroll'));
    }
    if (reason === 'multitouch') dispatchPointer(prose, 'pointerdown', { pointerId: 8, isPrimary: false, clientX: 50, clientY: 50 });
    if (reason === 'pointercancel') dispatchPointer(prose, 'pointercancel');
    vi.advanceTimersByTime(700);
    dispatchPointer(prose, 'contextmenu');
    expect(handler).not.toHaveBeenCalled();
    dispatchPointer(prose, 'pointerup');
    if (reason === 'multitouch') dispatchPointer(prose, 'pointerup', { pointerId: 8, isPrimary: false });
  });
});
