# 代码级验证用例与执行结果

方案接口以[00代码方案](00-综合结论与改造路线.md)为准。验证只围绕DOM接管、状态流转、同帧交接和导出。

## 1. 本次执行：7项契约断言通过

~~~powershell
node plan/vcpmobile-pele-streaming-research/evidence/contract-probe.mjs
~~~

[脚本](evidence/contract-probe.mjs)使用项目Vue、morphdom、happy-dom及固定Pele1.9.0；文字测量采用确定性宽度。Pele包位置读取已有[包证据](evidence/host-probe-1.9.0.json)中的scratch字段。断言为：

1. morph后宿主与SVG仍是原引用，source更新。
2. 非法前缀保留SVG和renderedSource配对。
3. finalized非法时切换error，隐藏图、显示源码。
4. source修正后恢复streaming和可见SVG。
5. 同tag宿主变普通div后sweep删除Entry。
6. Vue pre捕获模板，post先删除旧Entry再hydrate：缓存命中、零额外render。
7. Vue post收到不同source：同步render一次，nextTick结束前新SVG就位。

运行结果：7 passed，render调用7次，Node v24.14.1。此脚本检查方案契约和Vue提交时序；以下浏览器用例负责绘制帧。

## 2. 集成到现有测试的十个用例

| 用例 | 输入/操作 | 必须断言 |
|---|---|---|
| AST Replace | 连续两次同节点Mermaid replace | 宿主引用相同，viewport不被源码覆盖 |
| fallback | from有SVG，to为新source宿主 | guard写source并return false，SVG保留 |
| 删除/部分失败 | 先删图再令后续mutation失败；同tag改普通div | 批后entries清空相应项 |
| 流式语法 | 合法→半截边→合法 | 第二步保留old SVG；第三步SVG/source同时更新 |
| 终态非法 | 同一错误source从streaming转finalized | error状态、最新源码可见、旧图隐藏 |
| 首次加载 | 延迟import、更改source、删除旧host；先加载失败再由另一消息重试成功 | 现存host最新source可画；隐藏viewport不令宽度归零；成功清除loadError |
| promotion命中 | 闭围栏source/layout相同，关闭文字动画 | pre仍捕获；post新增render0次；第一提交态有SVG |
| promotion未命中 | 终态重分块、brk换父或source变化 | 同步重画；无旧source配新图；旧Entry清理 |
| 重复装饰/事件 | decorate三次；非法前缀时点击图 | 单wrapper/button；打开lastGood.source |
| SVG/PNG | 父级变量、classDef、marker；导出排队后关闭Viewer | 外部图颜色相同、引用完整；排队不再读取旧DOM |

AST与交互用例放入现有AstExecutorContinuity、MermaidMessageInteraction；同步模块替换MermaidRenderer旧initialize/串行队列断言。导出用例扩展MermaidGalleryExport。使用pnpm exec vitest run加这四个现有路径运行。

## 3. 浏览器首帧采样代码

在真实MessageRenderer fixture中预先生成可见tail SVG。commit函数同步接纳stable append/reset或终态投影；不在接管之后等待SVG出现再开始采样。

~~~ts
import { nextTick } from 'vue';

async function assertHandoffFrames(
  root: HTMLElement,
  source: string,
  commit: () => void,
): Promise<void> {
  const check = () => {
    const hosts = [...root.querySelectorAll<HTMLElement>(
      '[data-mermaid-host="1"]',
    )].filter(el => el.dataset.mermaidSource === source);
    const visible = hosts.filter(el => el.getBoundingClientRect().height > 0);
    if (visible.length !== 1) throw new Error('接管宿主丢失或重复');
    const view = visible[0].querySelector<HTMLElement>('[data-mermaid-viewport]')!;
    const code = visible[0].querySelector<HTMLElement>('[data-mermaid-source-view]')!;
    if (view.hidden || !view.querySelector('svg') || !code.hidden) {
      throw new Error('首个提交态出现源码或空白');
    }
  };

  await new Promise<void>((resolve, reject) => {
    requestAnimationFrame(() => {
      try { commit(); } catch (error) { reject(error); return; }
      void nextTick().then(async () => {
        check();
        for (let frame = 0; frame < 3; frame++) {
          await new Promise<void>(done => requestAnimationFrame(() => done()));
          check();
        }
        resolve();
      }).catch(reject);
    });
  });
}
~~~

分别用“缓存命中”“source不匹配同步重画”“brk重分块”“关闭文字动画”调用。测试前的tail SVG允许等待，commit之后的第一提交态必须立即检查。

## 4. SVG样式固化检查

在父节点设置--pele-fg/--pele-surface，自定义classDef颜色；用serializeDisplayedSvg得到独立SVG，放入无项目CSS的iframe和Image/Canvas。比较标签颜色、节点填充和字号，检查每个url(#id)目标存在。再添加同文档绝对marker URL和other.svg#id：前者本地化，后者原样保留。用arrow:head作为ID再经过instantiateMermaidSvg，确认百分号编码片段可解码并命中新ID。

核心用例完成后执行pnpm check与pnpm build。
