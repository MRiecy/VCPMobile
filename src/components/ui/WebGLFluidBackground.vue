<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue';
import { useThemeStore } from '../../core/stores/theme';

const themeStore = useThemeStore();
const canvasRef = ref<HTMLCanvasElement | null>(null);

let gl: WebGLRenderingContext | null = null;
let program: WebGLProgram | null = null;
let animationFrameId = 0;
let startTime = 0;
let resizeObserver: ResizeObserver | null = null;

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

const initWebGL = () => {
  const canvas = canvasRef.value;
  if (!canvas) return;

  gl = canvas.getContext('webgl', { 
    alpha: false, 
    antialias: true,
    depth: false,
    stencil: false,
    powerPreference: 'high-performance'
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
  renderLoop();
};

const renderLoop = () => {
  if (!gl || !program) return;

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

  animationFrameId = requestAnimationFrame(renderLoop);
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

onMounted(() => {
  // Let the browser settle before compiling shader
  setTimeout(() => {
    initWebGL();

    const canvas = canvasRef.value;
    if (canvas) {
      // 坚如磐石的 ResizeObserver，完美解决 SlidePage 缓动滑入期间宽度/高度动态变更导致的尺寸锁定问题
      resizeObserver = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const { width, height } = entry.contentRect;
          const dpr = Math.min(window.devicePixelRatio || 1, 2);
          canvas.width = Math.floor(width * dpr);
          canvas.height = Math.floor(height * dpr);
          if (gl) {
            gl.viewport(0, 0, canvas.width, canvas.height);
          }
        }
      });
      resizeObserver.observe(canvas.parentElement || canvas);

      // Zero-intrusive Parent Interaction Listener
      if (canvas.parentElement) {
        boundParent = canvas.parentElement;
        boundParent.addEventListener('mousemove', trackInteraction);
        boundParent.addEventListener('mouseleave', releaseInteraction);
        boundParent.addEventListener('touchmove', trackInteraction, { passive: false });
        boundParent.addEventListener('touchend', releaseInteraction);
        boundParent.addEventListener('mousedown', trackInteraction);
        boundParent.addEventListener('touchstart', trackInteraction, { passive: true });
      }
    }
  }, 100);
});

onUnmounted(() => {
  if (animationFrameId) {
    cancelAnimationFrame(animationFrameId);
  }
  
  if (resizeObserver) {
    resizeObserver.disconnect();
  }

  // Clean up event listeners on parent
  if (boundParent) {
    boundParent.removeEventListener('mousemove', trackInteraction);
    boundParent.removeEventListener('mouseleave', releaseInteraction);
    boundParent.removeEventListener('touchmove', trackInteraction);
    boundParent.removeEventListener('touchend', releaseInteraction);
    boundParent.removeEventListener('mousedown', trackInteraction);
    boundParent.removeEventListener('touchstart', trackInteraction);
  }

  if (gl && program) {
    const ext = gl.getExtension('WEBGL_lose_context');
    if (ext) {
      ext.loseContext(); // Release GPU memory instantly
    }
  }
  gl = null;
  program = null;
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
  <canvas 
    ref="canvasRef" 
    class="w-full h-full block bg-transparent pointer-events-none"
  />
</template>
