import { onUnmounted } from 'vue';

// Globals to track loaded scripts to avoid redundant fetches
if (!(window as any)._vcp_loaded_scripts) {
  (window as any)._vcp_loaded_scripts = new Set<string>();
}

export function useVcpMagic() {
  const trackedThreeInstances = new Map<HTMLElement, any[]>();
  const MAX_TRACKED_THREE = 20;
  let isThreePatched = false;

  const replaceCdnUrls = (scriptContent: string) => {
    if (!scriptContent) return scriptContent;
    let processed = scriptContent;

    // Map CDNs to local or fast CDNs that are mobile friendly. 
    // In VCPMobile, we might still rely on unpkg/jsdelivr since we don't pack them locally yet.
    // We'll rewrite older links to modern jsdelivr ones just in case.
    const threeJsPatterns = [
      /https?:\/\/cdnjs\.cloudflare\.com\/ajax\/libs\/three\.js\/[^'"`);\s]*/gi,
      /https?:\/\/unpkg\.com\/three[@\/][^'"`);\s]*/gi,
    ];
    threeJsPatterns.forEach(pattern => {
      processed = processed.replace(pattern, 'https://cdn.jsdelivr.net/npm/three@0.158.0/build/three.min.js');
    });

    const animeJsPatterns = [
      /https?:\/\/cdnjs\.cloudflare\.com\/ajax\/libs\/animejs\/[^'"`);\s]*/gi,
      /https?:\/\/unpkg\.com\/animejs[@\/][^'"`);\s]*/gi,
    ];
    animeJsPatterns.forEach(pattern => {
      processed = processed.replace(pattern, 'https://cdn.jsdelivr.net/npm/animejs@3.2.1/lib/anime.min.js');
    });

    return processed;
  };

  const loadScript = (src: string): Promise<void> => {
    return new Promise((resolve, reject) => {
      if ((window as any)._vcp_loaded_scripts.has(src)) {
        resolve();
        return;
      }
      (window as any)._vcp_loaded_scripts.add(src);

      const scriptEl = document.createElement('script');
      scriptEl.src = src;
      scriptEl.onload = () => {
        console.log(`[VCPMagic] ✅ Library loaded: ${src}`);
        resolve();
      };
      scriptEl.onerror = () => {
        console.error(`[VCPMagic] ❌ Failed to load: ${src}`);
        (window as any)._vcp_loaded_scripts.delete(src);
        reject(new Error(`Failed to load script ${src}`));
      };
      document.head.appendChild(scriptEl);
    });
  };

  const patchThreeJS = () => {
    if (isThreePatched || !(window as any).THREE || !(window as any).THREE.WebGLRenderer) return;

    const OriginalWebGLRenderer = (window as any).THREE.WebGLRenderer;

    (window as any).THREE.WebGLRenderer = function (...args: any[]) {
      const renderer = new OriginalWebGLRenderer(...args);
      const originalRender = renderer.render;
      let associatedScene: any = null;
      let associatedCamera: any = null;
      let isVisible = true;

      // 使用 IntersectionObserver 检测是否位于视口内
      const io = new IntersectionObserver((entries) => {
        isVisible = entries[0]?.isIntersecting ?? true;
      }, { threshold: 0 });
      if (renderer.domElement) {
        io.observe(renderer.domElement);
      }

      renderer.render = function (scene: any, camera: any) {
        if (this._disposed) return;

        // 页面在后台或元素不在视口内时跳过渲染，节省 GPU
        if (document.hidden || !isVisible) return;

        if (scene && !associatedScene) associatedScene = scene;
        if (camera && !associatedCamera) associatedCamera = camera;

        if (!document.body.contains(this.domElement)) {
          io.disconnect();
          if (!this._disposed) this.dispose();
          return;
        }

        try {
          return originalRender.call(this, scene, camera);
        } catch (error) {
          console.error('[Three.js Safety] Render error caught:', error);
          if (!this._disposed) this.dispose();
          return;
        }
      };

      const originalDispose = renderer.dispose;
      renderer.dispose = function () {
        if (this._disposed) return;
        this._disposed = true;
        if (originalDispose) {
          return originalDispose.call(this);
        }
      };

      const observer = new MutationObserver(() => {
        if (document.body.contains(renderer.domElement)) {
          const contentDiv = renderer.domElement.closest('.vcp-markdown-inner') as HTMLElement;
          if (contentDiv) {
            if (!trackedThreeInstances.has(contentDiv)) {
              trackedThreeInstances.set(contentDiv, []);
            }
            trackedThreeInstances.get(contentDiv)!.push({ renderer, getScene: () => associatedScene });
            // 防御性清理：防止 Map 无界增长
            if (trackedThreeInstances.size > MAX_TRACKED_THREE) {
              const first = trackedThreeInstances.keys().next().value;
              if (first) trackedThreeInstances.delete(first);
            }
          }
          observer.disconnect();
        }
      });

      observer.observe(document.body, { childList: true, subtree: true });

      return renderer;
    };

    (window as any).THREE.WebGLRenderer.prototype = OriginalWebGLRenderer.prototype;
    isThreePatched = true;
    console.log('[Three.js Patch] THREE.WebGLRenderer patched with safety checks.');
  };

  const scopeCss = (cssString: string, scopeId: string) => {
    let css = cssString.replace(/\/\*[\s\S]*?\*\//g, '');
    const rules = [];
    let depth = 0;
    let currentRule = '';

    for (let i = 0; i < css.length; i++) {
      const char = css[i];
      currentRule += char;
      if (char === '{') depth++;
      else if (char === '}') {
        depth--;
        if (depth === 0) {
          rules.push(currentRule.trim());
          currentRule = '';
        }
      }
    }

    return rules.map(rule => {
      const match = rule.match(/^([^{]+)\{(.+)\}$/s);
      if (!match) return rule;
      const [, selectors, body] = match;
      const scopedSelectors = selectors.split(',').map(s => {
        const sel = s.trim();
        if (sel.match(/^(@|from|to|\d+%|:root|html|body)/)) return sel;
        if (sel.match(/^::?[\w-]+$/)) return `.${scopeId}${sel}`;
        return `.${scopeId} ${sel}`;
      }).join(', ');

      return `${scopedSelectors} { ${body} }`;
    }).join('\n');
  };

  const processMagic = async (containerElement: HTMLElement, messageId: string) => {
    if (!containerElement) return;

    // 1. Process Styles
    const styles = Array.from(containerElement.querySelectorAll('style'));
    styles.forEach((style, index) => {
      // 如果样式已经由 useContentProcessor 处理过（带有 data-vcp-scoped），则跳过重写
      if (style.dataset.vcpScoped) {
        document.head.appendChild(style.cloneNode(true));
        style.remove();
        return;
      }

      const scopeId = `vcp-scope-${messageId}-${index}`;
      containerElement.classList.add(scopeId);
      const scopedContent = scopeCss(style.innerHTML, scopeId);
      const newStyle = document.createElement('style');
      newStyle.innerHTML = scopedContent;
      newStyle.dataset.vcpScope = scopeId;
      document.head.appendChild(newStyle);
      style.remove(); // Remove original
    });

    // 2. Process Interactive Buttons
    const buttons = Array.from(containerElement.querySelectorAll('button'));
    buttons.forEach(button => {
      if (button.dataset.vcpInteractive === 'true') return;
      button.dataset.vcpInteractive = 'true';
      button.style.cursor = 'pointer';
      button.type = 'button';
      button.setAttribute('type', 'button');

      button.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        e.stopImmediatePropagation();

        if (button.disabled) return false;

        const sendText = button.dataset.send || button.textContent?.trim();
        if (!sendText) return false;

        let finalSendText = `[[点击按钮:${sendText}]]`;
        if (finalSendText.length > 500) {
          finalSendText = `[[点击按钮:${sendText.substring(0, 500 - 11)}]]`;
        }

        // Disable button visually
        button.disabled = true;
        button.style.opacity = '0.6';
        button.style.cursor = 'not-allowed';
        button.dataset.originalText = button.textContent || '';
        button.textContent = (button.textContent || '') + ' ✓';

        // Trigger chat store send message
        setTimeout(() => {
          try {
            // Wait for chat store injection or global event
            const chatEvent = new CustomEvent('vcp-button-click', { detail: { text: finalSendText } });
            window.dispatchEvent(chatEvent);
          } catch (error) {
            console.error('[VCPMagic] Failed to send button message:', error);
            button.disabled = false;
            button.style.opacity = '1';
            button.style.cursor = 'pointer';
            if (button.dataset.originalText) {
              button.textContent = button.dataset.originalText;
            }
          }
        }, 10);
        return false;
      });
    });

    // 3. Highlight Text Patterns (Tags, Quotes, etc. after DOM is ready)
    highlightTextPatterns(containerElement);

    // 4. Process Scripts
    const allScripts = Array.from(containerElement.querySelectorAll('script'));
    const threeScripts = allScripts.filter(s => s.src && s.src.includes('three'));
    const otherExternalScripts = allScripts.filter(s => s.src && !s.src.includes('three'));
    const inlineScripts = allScripts.filter(s => !s.src && s.textContent?.trim());

    // Clean up all script tags from DOM
    allScripts.forEach(s => s.remove());

    const executeInline = () => {
      // Document API Shadowing to prevent document.write erasing the app
      const originalWrite = document.write;
      const originalOpen = document.open;
      const originalClose = document.close;

      const blockedApiHandler = function (...args: any[]) {
        console.warn('[VCPMagic] Blocked document.write/open/close:', args);
      };

      document.write = blockedApiHandler as any;
      document.open = blockedApiHandler as any;
      document.close = blockedApiHandler as any;

      try {
        inlineScripts.forEach((script, idx) => {
          try {
            // Generate a unique ID to help scripts target their container
            const containerId = `vcp-magic-container-${messageId}-${idx}`;
            containerElement.id = containerId;

            let scriptContent = script.textContent || '';

            // Auto-inject a variable `container` to help AI scripts find their root
            // 同时支持桌面端常用的 #vcp-root 查找逻辑
            const wrappedScript = `
(function() {
  const container = document.getElementById('${containerId}');
  if (!container) return;
  
  // 兼容性补丁：让脚本可以通过 querySelector('#vcp-root') 找到自己
  const vcpRoot = container.querySelector('#vcp-root') || container;
  
  try {
    // 在 IIFE 作用域内注入常用变量
    const root = vcpRoot;
    ${scriptContent}
  } catch (e) {
    console.error('[VCPMagic] Error in AI script:', e);
  }
})();`;

            const newScript = document.createElement('script');
            newScript.textContent = wrappedScript;
            document.head.appendChild(newScript).parentNode?.removeChild(newScript);
          } catch (e) {
            console.error('[VCPMagic] Error executing inline script:', e);
          }
        });
      } finally {
        document.write = originalWrite;
        document.open = originalOpen;
        document.close = originalClose;
      }
    };

    try {
      if (threeScripts.length > 0) {
        await loadScript('https://cdn.jsdelivr.net/npm/three@0.158.0/build/three.min.js');
        patchThreeJS();
      }

      for (const s of otherExternalScripts) {
        if (s.src) {
          await loadScript(replaceCdnUrls(s.src));
        }
      }

      executeInline();
    } catch (e) {
      console.error('[VCPMagic] Failed to load external dependencies', e);
    }
  };

  const highlightTextPatterns = (containerElement: HTMLElement) => {
    if (!containerElement) return;

    const TAG_REGEX = /@([\u4e00-\u9fa5A-Za-z0-9_]+)/g;
    const ALERT_TAG_REGEX = /@!([\u4e00-\u9fa5A-Za-z0-9_]+)/g;
    const BOLD_REGEX = /\*\*([^\*]+)\*\*/g;
    const QUOTE_REGEX = /(?:"([^"]*)"|“([^”]*)”)/g; // Matches English "..." and Chinese “...”

    const walker = document.createTreeWalker(
      containerElement,
      NodeFilter.SHOW_TEXT,
      (node) => {
        let parent = node.parentElement;
        while (parent && parent !== containerElement) {
          if (
            ['PRE', 'CODE', 'STYLE', 'SCRIPT', 'STRONG', 'B'].includes(parent.tagName) ||
            parent.classList.contains('highlighted-tag') ||
            parent.classList.contains('highlighted-quote') ||
            parent.classList.contains('highlighted-alert-tag')
          ) {
            return NodeFilter.FILTER_REJECT;
          }
          parent = parent.parentElement;
        }
        return NodeFilter.FILTER_ACCEPT;
      }
    );

    const nodesToProcess: { node: Node; matches: { type: string; index: number; length: number; content: string }[] }[] = [];
    let node;

    try {
      while ((node = walker.nextNode())) {
        const text = node.nodeValue || '';
        if (!text) continue;
        const matches: { type: string; index: number; length: number; content: string }[] = [];

        let match;
        while ((match = TAG_REGEX.exec(text)) !== null) {
          matches.push({ type: 'tag', index: match.index, length: match[0].length, content: match[0] });
        }
        while ((match = ALERT_TAG_REGEX.exec(text)) !== null) {
          matches.push({ type: 'alert-tag', index: match.index, length: match[0].length, content: match[0] });
        }
        while ((match = BOLD_REGEX.exec(text)) !== null) {
          matches.push({ type: 'bold', index: match.index, length: match[0].length, content: match[1] });
        }
        while ((match = QUOTE_REGEX.exec(text)) !== null) {
          if (match[1] || match[2]) {
            matches.push({ type: 'quote', index: match.index, length: match[0].length, content: match[0] });
          }
        }

        if (matches.length > 0) {
          matches.sort((a, b) => a.index - b.index);
          nodesToProcess.push({ node, matches });
        }
      }
    } catch (error) {
      if (!(error instanceof Error && error.message.includes("no longer runnable"))) {
        console.error("[VCPMagic] TreeWalker error", error);
      }
    }

    // Process nodes in reverse order to avoid messing up indices
    for (let i = nodesToProcess.length - 1; i >= 0; i--) {
      const { node, matches } = nodesToProcess[i];
      if (!node.parentNode || !node.nodeValue) continue;

      const filteredMatches = [];
      let lastIndexProcessed = -1;
      for (const currentMatch of matches) {
        if (currentMatch.index >= lastIndexProcessed) {
          filteredMatches.push(currentMatch);
          lastIndexProcessed = currentMatch.index + currentMatch.length;
        }
      }

      if (filteredMatches.length === 0) continue;

      const fragment = document.createDocumentFragment();
      let lastIndex = 0;

      filteredMatches.forEach(match => {
        if (match.index > lastIndex) {
          fragment.appendChild(document.createTextNode(node.nodeValue!.substring(lastIndex, match.index)));
        }

        const span = document.createElement(match.type === 'bold' ? 'strong' : 'span');
        if (match.type === 'tag') {
          span.className = 'highlighted-tag';
          span.textContent = match.content;
        } else if (match.type === 'alert-tag') {
          span.className = 'highlighted-alert-tag';
          span.textContent = match.content;
        } else if (match.type === 'quote') {
          span.className = 'highlighted-quote';
          span.textContent = match.content;
        } else { // bold
          // 兜底重绘出来的未闭合加粗，我们包裹原生 strong 标签，
          // 因为在 CSS 里我们已经用 .vcp-markdown-block strong { color: ... } 劫持了！
          span.textContent = match.content;
        }
        fragment.appendChild(span);

        lastIndex = match.index + match.length;
      });

      if (lastIndex < node.nodeValue.length) {
        fragment.appendChild(document.createTextNode(node.nodeValue.substring(lastIndex)));
      }

      node.parentNode.replaceChild(fragment, node);
    }
  };

  const cleanupMagic = (containerElement: HTMLElement | null) => {
    if (!containerElement) return;

    // Cleanup anime.js animations
    if ((window as any).anime) {
      const animatedElements = containerElement.querySelectorAll('*');
      if (animatedElements.length > 0) {
        (window as any).anime.remove(animatedElements);
      }
    }

    // Cleanup Three.js contexts
    if (trackedThreeInstances.has(containerElement)) {
      const instances = trackedThreeInstances.get(containerElement)!;
      instances.forEach(instance => {
        if (instance.renderer && !instance.renderer._disposed) {
          const scene = instance.getScene();
          if (scene) {
            scene.traverse((object: any) => {
              if (object.isMesh) {
                if (object.geometry) object.geometry.dispose();
                if (object.material) {
                  if (Array.isArray(object.material)) {
                    object.material.forEach((mat: any) => mat.dispose && mat.dispose());
                  } else if (object.material.dispose) {
                    object.material.dispose();
                  }
                }
              }
            });
          }
          try {
            instance.renderer.dispose();
          } catch (e) { }
        }
      });
      trackedThreeInstances.delete(containerElement);
    }

    // Cleanup scoped CSS
    const classList = Array.from(containerElement.classList);
    classList.forEach(cls => {
      if (cls.startsWith('vcp-scope-')) {
        const styleTag = document.querySelector(`style[data-vcp-scope="${cls}"]`);
        if (styleTag) styleTag.remove();
      }
    });
  };

  onUnmounted(() => {
    // If the component using this composable unmounts, we should ideally clean up, 
    // but the actual cleanup needs the specific container element. 
    // This is handled per-message in MessageRenderer.
  });

  return {
    processMagic,
    cleanupMagic
  };
}
