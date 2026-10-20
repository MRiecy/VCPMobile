import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { isAndroid } from '../../../core/utils/platform';
import { defineGuide } from '../registry';

// Android 使用系统 WebView；其他平台不注册这条说明或回放入口。
if (isAndroid) {
  defineGuide({
    id: 'android-webview-compatibility',
    title: 'Android WebView 兼容说明',
    description: '手机和平板均需 WebView 117 或更新内核',
    trigger: {
      predicates: [{
        name: 'workspace-ready',
        check: () => useAppLifecycleStore().state === 'READY',
      }],
    },
    steps: [{
      target: null,
      title: '请保持系统 WebView 更新',
      content: 'Android 手机和平板都需要 Chromium 117 或更新版本的系统 WebView，才能正常显示和操作 VCP Mobile。\n\n'
        + 'Android 系统版本不等于 WebView 内核版本。请通过系统更新、设备厂商或官方应用商店，更新设备实际使用的 WebView 组件。\n\n'
        + '更新 Chrome 浏览器不一定会更新应用使用的内核。此说明可在设置中的「帮助与指引」重新查看。',
    }],
  });
}
