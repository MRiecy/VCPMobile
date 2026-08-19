// 层级铁律：reset/preflight 必须在 uno.css 之前。
// 反序时 reset 里的 [type='button'] 等属性选择器与单类工具同优先级 (0,1,0)，
// 靠后出现而覆盖 bg-*/text-* 工具类（真实事故：RefreshButton 带 type="button"
// 后 bg-black/5 被透明掉，而无 type 属性的原生按钮不受影响）。
import "@unocss/reset/tailwind.css";
import "virtual:uno.css";
import "./assets/themes.css";
