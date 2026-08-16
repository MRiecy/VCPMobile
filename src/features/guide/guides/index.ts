/**
 * 指引定义注册入口
 *
 * 由 GuideOverlay 在模块加载期 import（先于 guideStore.init() 的
 * 触发评估），保证所有指引定义在 watchEffect 首轮评估前注册完毕。
 */
import './sidebar-gestures';
import './theme-longpress';
import './plus-longpress';
import './diary-longpress';
