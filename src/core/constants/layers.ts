/**
 * VCP 全局层级常量系统
 *
 * 用途：统一所有覆盖层、抽屉、弹窗、提示等 UI 元素的 z-index 层级，
 * 消除魔法数字，建立可维护、可扩展的层级秩序。
 *
 * 层级从低到高：
 *   content  (0)   → 页面内容、气泡、列表
 *   local    (10)  → 页面内局部悬浮（置底按钮、角标、hover覆盖）
 *   drawer   (20)  → 左右抽屉 + 遮罩
 *   overlay  (30)  → 全局覆盖容器（GlobalOverlayManager）
 *   page     (40+) → SlidePage 页面栈（40 + stackIndex）
 *   toast    (50)  → Toast 通知
 *   dialog   (60)  → Prompt、UpdatePrompt、ContextMenu
 *   sheet    (70)  → BottomSheet、ModelSelector
 *   viewer   (80)  → AttachmentViewer、FullScreenEditor、AvatarCropper
 *   editor   (90)  → HtmlPreviewBlock（全屏HTML）
 *   boot     (100) → BootScreen（启动屏）
 */

export const LAYER_CONTENT = 0;
export const LAYER_LOCAL = 10;
export const LAYER_DRAWER = 20;
export const LAYER_OVERLAY = 30;
export const LAYER_PAGE_BASE = 40;
export const LAYER_TOAST = 50;
export const LAYER_DIALOG = 60;
export const LAYER_SHEET = 70;
export const LAYER_VIEWER = 80;
export const LAYER_EDITOR = 90;
export const LAYER_BOOT = 100;

/** 页面栈最多支持的层数（预留到 49，封顶） */
export const LAYER_PAGE_MAX_OFFSET = 9;

/**
 * 计算页面栈中某页面的动态 z-index
 * @param stackIndex 页面在栈中的索引（0-based）
 */
export const getPageZIndex = (stackIndex: number): number => {
  const offset = Math.min(stackIndex, LAYER_PAGE_MAX_OFFSET);
  return LAYER_PAGE_BASE + offset;
};

/** 所有层级的名称类型 */
export type LayerName =
  | 'content'
  | 'local'
  | 'drawer'
  | 'overlay'
  | 'page'
  | 'toast'
  | 'dialog'
  | 'sheet'
  | 'viewer'
  | 'editor'
  | 'boot';

/** 层级名称到数值的映射 */
export const LAYER_MAP: Record<LayerName, number> = {
  content: LAYER_CONTENT,
  local: LAYER_LOCAL,
  drawer: LAYER_DRAWER,
  overlay: LAYER_OVERLAY,
  page: LAYER_PAGE_BASE,
  toast: LAYER_TOAST,
  dialog: LAYER_DIALOG,
  sheet: LAYER_SHEET,
  viewer: LAYER_VIEWER,
  editor: LAYER_EDITOR,
  boot: LAYER_BOOT,
};
