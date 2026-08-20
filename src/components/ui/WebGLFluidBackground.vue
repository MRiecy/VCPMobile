<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue';
import { useThemeStore } from '../../core/stores/theme';

// 常驻型流体背景宿主（空间换时间设计）：
// - 挂载后经 requestIdleCallback 预热：创建 WebGL 上下文并完成着色器编译
//  （KHR_parallel_shader_compile 可用时全程异步轮询，绝不阻塞主线程），
//   编译产物之后永久持有，About 页打开即热；
// - setActive(true/false) 控制 rAF 渲染循环与交互监听，About 页关闭即停，空闲零 GPU 开销；
// - 冷路径（预热未完成就打开 About）：主题化 CSS 极光渐变兜底，首帧绘制完成后画布交叉淡入，
//   用户全程看不到黑屏或动画冻结。
const themeStore = useThemeStore();
const hostRef = ref<HTMLElement | null>(null);
const canvasRef = ref<HTMLCanvasElement | null>(null);

/** 着色器编译链接完成，随时可以开始渲染 */
const ready = ref(false);
/** 激活后已绘制至少一帧 —— 驱动画布从兜底渐变上交叉淡入 */
const firstFrame = ref(false);

let gl: WebGLRenderingContext | null = null;
let program: WebGLProgram | null = null;
let animationFrameId = 0;
let startTime = 0;
let resizeObserver: ResizeObserver | null = null;
let active = false;
let warmStarted = false;

// WebGL uniform locations
let uResolutionLoc: WebGLUniformLocation | null = null;
let uTimeLoc: WebGLUniformLocation | null = null;
let uMouseLoc: WebGLUniformLocation | null = null;
let uActiveLoc: WebGLUniformLocation | null = null;
let uIsDarkLoc: WebGLUniformLocation | null = null;

// Physics and interaction state
const mousePos = { x: 0.5, y: 0.5 };
const targetMousePos = { x: 0.5, y: 0.5 };
let activeValue = 0.0;
let targetActiveValue = 0.0;

// Vertex Shader: Renders a screen-filling quad
const vsSource = `
  attribute vec2 position;
  void main() {
    gl_Position = vec4(position, 0.0, 1.0);
  }
`;

// Fragment Shader: Horizontally spread Width-Normalized Anisotropic Gaussian Aurora Ribbon
const fsSource = `
  precision highp float;
  uniform vec2 u_resolution;
  uniform float u_time;
  uniform vec2 u_mouse;
  uniform float u_active;
  uniform float u_is_dark;

  // Anisotropic Gaussian weight calculation in Width-Normalized space
  // Ensures extreme horizontal blur and strict vertical containment
  float anisotropic_gaussian(vec2 uv_diff, float aspect, float sigma_x, float sigma_y) {
    float dx = uv_diff.x;
    float dy = uv_diff.y * aspect;
    return exp(-0.5 * ( (dx * dx) / (sigma_x * sigma_x) + (dy * dy) / (sigma_y * sigma_y) ));
  }

  // Width-Normalized Isotropic Repulsion physics (100% resolution invariant)
  vec2 repel(vec2 p, vec2 mouse, float active, float aspect) {
    vec2 aspect_p = vec2(p.x, p.y * aspect);
    vec2 aspect_mouse = vec2(mouse.x, mouse.y * aspect);
    vec2 to_p = aspect_p - aspect_mouse;
    float dist = length(to_p);
    
    // Interactive force field: 25% screen width radius, 15% screen width max push displacement
    float force = 0.15 * active * exp(-(dist * dist) / (0.25 * 0.25));
    if (dist > 0.001) {
      vec2 new_aspect_p = aspect_p + normalize(to_p) * force;
      return vec2(new_aspect_p.x, new_aspect_p.y / aspect); // Map back to 0..1 viewport space
    }
    return p;
  }

  void main() {
    // 1. Domain Warping for fluid Aerogel refraction (subtle, clean, gel-like cohesive tension)
    // Displaces UV slightly using sine waves to create organic gel boundaries instead of hard geometric ovals
    vec2 warped_uv = gl_FragCoord.xy / u_resolution.xy;
    float warp_strength = mix(0.012, 0.022, u_active); // Strengthens slightly during touch warp
    warped_uv.x += sin(warped_uv.y * 14.0 + u_time * 0.95) * warp_strength;
    warped_uv.y += cos(warped_uv.x * 12.0 + u_time * 0.82) * warp_strength;

    float aspect = u_resolution.y / u_resolution.x;
    vec2 mouse_p = u_mouse;

    // 2. Basic trigonometric orbits (normalized 0..1 workspace)
    vec2 p1 = vec2(0.18, 0.81) + vec2(sin(u_time * 0.92) * 0.08, cos(u_time * 0.68) * 0.04);
    vec2 p2 = vec2(0.82, 0.77) + vec2(cos(u_time * 0.68) * 0.08, sin(u_time * 0.91) * 0.06);
    vec2 p3 = vec2(0.36, 0.72) + vec2(sin(u_time * 0.55) * 0.06, cos(u_time * 1.06) * 0.03);
    vec2 p4 = vec2(0.64, 0.82) + vec2(cos(u_time * 0.85) * 0.07, sin(u_time * 0.65) * 0.05);

    // 3. Isotropic Repulsion physics (Width-Normalized)
    p1 = repel(p1, mouse_p, u_active, aspect);
    p2 = repel(p2, mouse_p, u_active, aspect);
    p3 = repel(p3, mouse_p, u_active, aspect);
    p4 = repel(p4, mouse_p, u_active, aspect);

    // 4. Compute Anisotropic Gaussian weights using warped coordinates
    // Creates refracting, refracting liquid-gel margins
    float w1 = anisotropic_gaussian(warped_uv - p1, aspect, 0.48, 0.18);
    float w2 = anisotropic_gaussian(warped_uv - p2, aspect, 0.55, 0.22);
    float w3 = anisotropic_gaussian(warped_uv - p3, aspect, 0.45, 0.17);
    float w4 = anisotropic_gaussian(warped_uv - p4, aspect, 0.58, 0.20);

    // Brand Fluid Colors definition
    vec3 col_cyan = vec3(0.0, 0.88, 1.0);    
    vec3 col_magenta = vec3(1.0, 0.20, 0.45);
    vec3 col_violet = vec3(0.66, 0.33, 0.97);
    vec3 col_blue = vec3(0.11, 0.30, 0.85);  

    // 5. Blend background depending on Light/Dark active mode
    vec3 bg_light = vec3(0.97, 0.98, 0.99); // #f8fafc slate-50
    vec3 bg_dark = vec3(0.06, 0.09, 0.16);   // #0f172a slate-900
    vec3 base_bg = mix(bg_light, bg_dark, u_is_dark);

    // Vignette mask calculated using warped UV to follow fluid flow borders
    vec2 center_uv = warped_uv - vec2(0.5);
    float vignette = smoothstep(0.72, 0.15, length(center_uv));

    // 6. Gaseous Optical Layering Blending (正向气态逐层光学叠染)
    // 100% direct positive mixing: ensures Cyan is Cyan, Magenta is Magenta, Violet is Violet, Blue is Blue.
    // Discards complementary projection to completely eliminate toxic moldy brown/green color shifts!
    float intensity_cyan = mix(0.38, 0.48, u_is_dark);
    float intensity_magenta = mix(0.35, 0.45, u_is_dark);
    float intensity_violet = mix(0.36, 0.46, u_is_dark);
    float intensity_blue = mix(0.40, 0.52, u_is_dark);

    vec3 color = base_bg;
    color = mix(color, col_cyan, w1 * intensity_cyan * vignette);
    color = mix(color, col_violet, w3 * intensity_violet * vignette);
    color = mix(color, col_blue, w4 * intensity_blue * vignette);
    color = mix(color, col_magenta, w2 * intensity_magenta * vignette);

    gl_FragColor = vec4(color, 1.0);
  }
`;

const createShader = (gl: WebGLRenderingContext, type: number, source: string): WebGLShader | null => {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    console.error('[WebGLShader] Shader compile error:', gl.getShaderInfoLog(shader));
    gl.deleteShader(shader);
    return null;
  }
  return shader;
};

/** 链接完成后的收尾：建立顶点缓冲与 uniform 定位，标记就绪并按需启动渲染循环 */
const finishLink = () => {
  if (!gl || !program) return;
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.error('[WebGL] Link error:', gl.getProgramInfoLog(program));
    return;
  }

  gl.useProgram(program);

  // Position Buffer (covering full NDC viewport space)
  const vertices = new Float32Array([
    -1.0, -1.0,
     1.0, -1.0,
    -1.0,  1.0,
    -1.0,  1.0,
     1.0, -1.0,
     1.0,  1.0,
  ]);

  const positionBuffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, positionBuffer);
  gl.bufferData(gl.ARRAY_BUFFER, vertices, gl.STATIC_DRAW);

  const positionLoc = gl.getAttribLocation(program, 'position');
  gl.enableVertexAttribArray(positionLoc);
  gl.vertexAttribPointer(positionLoc, 2, gl.FLOAT, false, 0, 0);

  // Uniform locations
  uResolutionLoc = gl.getUniformLocation(program, 'u_resolution');
  uTimeLoc = gl.getUniformLocation(program, 'u_time');
  uMouseLoc = gl.getUniformLocation(program, 'u_mouse');
  uActiveLoc = gl.getUniformLocation(program, 'u_active');
  uIsDarkLoc = gl.getUniformLocation(program, 'u_is_dark');

  startTime = performance.now();
  ready.value = true;
  if (active) startLoop();
};

/**
 * 预热（幂等）：创建 WebGL 上下文并编译链接着色器，但不启动渲染循环。
 * KHR_parallel_shader_compile 可用时异步轮询编译完成状态，主线程零阻塞；
 * 不可用时退化为同步等待——但 warm 只在空闲回调中被调用，不会卡任何进场动画。
 */
const warm = () => {
  if (warmStarted) return;
  warmStarted = true;

  const canvas = canvasRef.value;
  if (!canvas) return;

  gl = canvas.getContext('webgl', {
    alpha: false,
    antialias: false, // 程序化全屏 quad 无几何边缘，MSAA 纯属浪费上下文创建开销
    depth: false,
    stencil: false,
    powerPreference: 'default', // 环境光背景无需强制高性能 GPU 档位
  });

  if (!gl) {
    console.warn('[WebGL] WebGL context is not supported, falling back.');
    return;
  }

  const vs = createShader(gl, gl.VERTEX_SHADER, vsSource);
  const fs = createShader(gl, gl.FRAGMENT_SHADER, fsSource);
  if (!vs || !fs) return;

  program = gl.createProgram();
  if (!program) return;

  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);

  const parallelExt = gl.getExtension('KHR_parallel_shader_compile');
  if (parallelExt) {
    const poll = () => {
      if (!gl || !program) return;
      if (gl.getProgramParameter(program, parallelExt.COMPLETION_STATUS_KHR)) {
        finishLink();
      } else {
        requestAnimationFrame(poll);
      }
    };
    requestAnimationFrame(poll);
  } else {
    finishLink();
  }
};

const renderLoop = () => {
  if (!gl || !program || !active) return;

  const canvas = canvasRef.value;
  if (!canvas) return;

  // Smooth Interpolation of physical interactions (Ease-out logic)
  mousePos.x += (targetMousePos.x - mousePos.x) * 0.15;
  mousePos.y += (targetMousePos.y - mousePos.y) * 0.15;
  activeValue += (targetActiveValue - activeValue) * 0.12;

  // Clear & Setup uniforms
  gl.clearColor(0.0, 0.0, 0.0, 1.0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  gl.useProgram(program);

  // Upload uniform parameters to GPU
  gl.uniform2f(uResolutionLoc, canvas.width, canvas.height);
  gl.uniform1f(uTimeLoc, (performance.now() - startTime) / 1000.0);
  gl.uniform2f(uMouseLoc, mousePos.x, mousePos.y);
  gl.uniform1f(uActiveLoc, activeValue);
  gl.uniform1f(uIsDarkLoc, themeStore.isDarkResolved ? 1.0 : 0.0);

  // Draw full viewport quad
  gl.drawArrays(gl.TRIANGLES, 0, 6);
  if (!firstFrame.value) firstFrame.value = true;

  animationFrameId = requestAnimationFrame(renderLoop);
};

const startLoop = () => {
  if (animationFrameId) return;
  animationFrameId = requestAnimationFrame(renderLoop);
};

const stopLoop = () => {
  if (animationFrameId) {
    cancelAnimationFrame(animationFrameId);
    animationFrameId = 0;
  }
};

/** 按宿主实际尺寸重设画布分辨率；v-show 隐藏期间尺寸为 0，跳过等待下次激活 */
const resizeToHost = () => {
  const canvas = canvasRef.value;
  const host = hostRef.value;
  if (!canvas || !host) return;
  const rect = host.getBoundingClientRect();
  if (rect.width < 2 || rect.height < 2) return;
  // 极光为低频模糊内容，DPR 1.5 完全无法感知差异，却省去约 44% 的片元开销
  const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
  canvas.width = Math.floor(rect.width * dpr);
  canvas.height = Math.floor(rect.height * dpr);
  if (gl) {
    gl.viewport(0, 0, canvas.width, canvas.height);
  }
};

// Input event tracking helpers
const trackInteraction = (e: Event) => {
  const canvas = canvasRef.value;
  if (!canvas) return;

  const rect = canvas.getBoundingClientRect();
  let clientX = 0;
  let clientY = 0;

  if (window.TouchEvent && e instanceof TouchEvent) {
    if (e.touches.length === 0) return;
    clientX = e.touches[0].clientX;
    clientY = e.touches[0].clientY;
  } else if (e instanceof MouseEvent) {
    clientX = e.clientX;
    clientY = e.clientY;
  } else {
    return;
  }

  // Normalize position to 0.0 ~ 1.0 range (with flipped Y for WebGL)
  const normalizedX = (clientX - rect.left) / rect.width;
  const normalizedY = 1.0 - (clientY - rect.top) / rect.height;

  targetMousePos.x = Math.max(0.0, Math.min(1.0, normalizedX));
  targetMousePos.y = Math.max(0.0, Math.min(1.0, normalizedY));
  targetActiveValue = 1.0; // Trigger physical warp force
};

const releaseInteraction = () => {
  targetActiveValue = 0.0; // Slowly decay warp force
};

let boundParent: HTMLElement | null = null;

// 交互监听挂在宿主父元素上（宿主自身 pointer-events:none，事件从 About 内容层冒泡上来）。
// 仅激活期间持有监听，避免常驻监听整个设置页。
const bindInteraction = () => {
  const parent = hostRef.value?.parentElement;
  if (!parent || boundParent === parent) return;
  boundParent = parent;
  parent.addEventListener('mousemove', trackInteraction);
  parent.addEventListener('mouseleave', releaseInteraction);
  parent.addEventListener('touchmove', trackInteraction, { passive: false });
  parent.addEventListener('touchend', releaseInteraction);
  parent.addEventListener('mousedown', trackInteraction);
  parent.addEventListener('touchstart', trackInteraction, { passive: true });
};

const unbindInteraction = () => {
  if (!boundParent) return;
  boundParent.removeEventListener('mousemove', trackInteraction);
  boundParent.removeEventListener('mouseleave', releaseInteraction);
  boundParent.removeEventListener('touchmove', trackInteraction);
  boundParent.removeEventListener('touchend', releaseInteraction);
  boundParent.removeEventListener('mousedown', trackInteraction);
  boundParent.removeEventListener('touchstart', trackInteraction);
  boundParent = null;
};

// Android WebView 在内存压力下可能销毁 GPU 上下文：拦截丢失事件并标记未就绪，恢复后重新预热
const handleContextLost = (e: Event) => {
  e.preventDefault();
  stopLoop();
  ready.value = false;
  firstFrame.value = false;
  warmStarted = false;
  gl = null;
  program = null;
};

const handleContextRestored = () => {
  warm();
};

/**
 * 激活/停用渲染循环。激活时若尚未预热（冷路径）立即触发预热——
 * 并行编译扩展保证这不会阻塞主线程，兜底渐变遮住编译窗口。
 */
const setActive = (value: boolean) => {
  if (value === active) return;
  active = value;
  if (value) {
    if (!ready.value) warm();
    bindInteraction();
    // v-show 刚切为可见时宿主尺寸尚未布局，顺延一帧再测量
    requestAnimationFrame(() => {
      resizeToHost();
      if (ready.value) startLoop();
    });
  } else {
    stopLoop();
    unbindInteraction();
  }
};

defineExpose({ setActive, warm });

onMounted(() => {
  const canvas = canvasRef.value;
  const host = hostRef.value;
  if (!canvas || !host) return;

  canvas.addEventListener('webglcontextlost', handleContextLost);
  canvas.addEventListener('webglcontextrestored', handleContextRestored);

  resizeObserver = new ResizeObserver(() => {
    if (active) resizeToHost();
  });
  resizeObserver.observe(host);

  // 空闲预热：着色器编译产物常驻内存，About 页打开即热（空间换时间）。
  // 与 theme.ts initTheme 的空闲加载模式同构。
  const idle: (cb: () => void) => void =
    (window as any).requestIdleCallback?.bind(window) || ((cb) => setTimeout(cb, 800));
  idle(() => warm());
});

onUnmounted(() => {
  stopLoop();
  unbindInteraction();

  if (resizeObserver) {
    resizeObserver.disconnect();
  }

  const canvas = canvasRef.value;
  if (canvas) {
    canvas.removeEventListener('webglcontextlost', handleContextLost);
    canvas.removeEventListener('webglcontextrestored', handleContextRestored);
  }

  gl = null;
  program = null;
});

// 编译完成时若已处于激活态（冷路径），立即开始渲染
watch(ready, (isReady) => {
  if (isReady && active) {
    resizeToHost();
    startLoop();
  }
});

// Proactively update dark mode value inside running loop when theme changes
watch(() => themeStore.isDarkResolved, (isDark) => {
  if (gl && program && uIsDarkLoc) {
    gl.useProgram(program);
    gl.uniform1f(uIsDarkLoc, isDark ? 1.0 : 0.0);
  }
});
</script>

<template>
  <div ref="hostRef" class="fluid-host absolute inset-0 overflow-hidden pointer-events-none">
    <!-- 冷路径兜底：主题化 CSS 极光渐变（轨道坐标与着色器一致），首帧就绪后被画布覆盖 -->
    <div
      class="fluid-fallback absolute inset-0"
      :class="themeStore.isDarkResolved ? '' : 'fluid-fallback-light'"
    />
    <canvas
      ref="canvasRef"
      class="absolute inset-0 w-full h-full block transition-opacity duration-500"
      :style="{ opacity: firstFrame ? 1 : 0 }"
    />
    <!-- 电影级胶片噪点层：与画布同处一个合成上下文，mix-blend 直接作用于流体画面 -->
    <div
      class="noise-overlay absolute inset-0 pointer-events-none overflow-hidden"
      :class="themeStore.isDarkResolved ? '' : 'light-mode-noise'"
    />
  </div>
</template>

<style scoped>
/* 兜底渐变：近似着色器中四个高斯光团的静态快照，仅在预热未完成的冷路径短暂可见 */
.fluid-fallback {
  background-color: #0f172a;
  background-image:
    radial-gradient(65% 45% at 18% 80%, rgba(0, 224, 255, 0.32), transparent 72%),
    radial-gradient(60% 45% at 82% 76%, rgba(255, 51, 109, 0.28), transparent 72%),
    radial-gradient(55% 40% at 38% 72%, rgba(168, 85, 247, 0.3), transparent 72%),
    radial-gradient(60% 42% at 64% 82%, rgba(29, 78, 216, 0.34), transparent 72%);
}

.fluid-fallback-light {
  background-color: #f8fafc;
  background-image:
    radial-gradient(65% 45% at 18% 80%, rgba(0, 224, 255, 0.22), transparent 72%),
    radial-gradient(60% 45% at 82% 76%, rgba(255, 51, 109, 0.18), transparent 72%),
    radial-gradient(55% 40% at 38% 72%, rgba(168, 85, 247, 0.2), transparent 72%),
    radial-gradient(60% 42% at 64% 82%, rgba(29, 78, 216, 0.2), transparent 72%);
}

/* 胶片颗粒噪点层，使用 0KB 纯 SVG 分形噪声物理抹除大渐变色带 */
.noise-overlay {
  opacity: 0.06; /* 从 0.045 提升至 0.06，增强像素打散强度以抵抗物理干涉，颗粒质感更细腻高级 */
  mix-blend-mode: overlay;
  background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noiseFilter'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noiseFilter)'/%3E%3C/svg%3E");
  transition: opacity 0.5s ease;
}

/* 亮色模式下的胶片颗粒 - 呈 multiply 模式，不透明度略降，提供 pure white 磨砂玻璃颗粒触感 */
.noise-overlay.light-mode-noise {
  mix-blend-mode: multiply;
  opacity: 0.038;
}
</style>
