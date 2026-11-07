/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 构建期可选注入的默认桌面桥接 Base（自部署发行版用；公共发行版留空）。 */
  readonly VITE_VCP_DEFAULT_BRIDGE_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  // Preserve Vue's generic SFC fallback; this does not declare a component with no allowed props.
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

declare module 'vue-cropper-next';
