/**
 * attachMenuController — 聊天框附件面板开关状态控制器
 *
 * 从 InputEnhancer.vue 提取的共享状态（原为组件本地 ref `showAttachMenu`）：
 * - 教学引导「单击 + 按钮」步骤需要真实展开附件面板（拍摄/相册/文件）；
 * - InputEnhancer 的点击切换与面板渲染继续读写同一 ref，行为不变。
 */
import { ref } from 'vue';

export const attachMenuOpen = ref(false);

export function toggleAttachMenu(): void {
  attachMenuOpen.value = !attachMenuOpen.value;
}

export function setAttachMenu(open: boolean): void {
  attachMenuOpen.value = open;
}
