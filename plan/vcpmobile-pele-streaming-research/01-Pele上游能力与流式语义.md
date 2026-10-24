# Pele 1.9.0：选定接口的源码依据

本方案使用import('pele')和同步render(source, options)。应用接口、缓存及状态机集中在[00代码方案第2节](00-综合结论与改造路线.md)。

## 1. 固定输入

| 项目 | 值 |
|---|---|
| 包版本 | pele@1.9.0 |
| 发布提交 | e7f092a4de147cc32210913c20e36eaa5fcd0ef1 |
| npm发布时间 | 北京时间2026-10-23 06:45 |
| 实际包SHA-256 | d9dd5bc81e8d4d5747a2af07d54e2b8b9e897bff49cc4af7612d219c3d70ba33 |
| 项目现用引擎 | mermaid 12.2.0 |
| 配置改动 | package.json将mermaid改为精确pele 1.9.0；pnpm更新lock |

来源：[固定npm元数据](https://registry.npmjs.org/pele/1.9.0)、[package.json](https://github.com/obsidianmd/pele/blob/e7f092a4de147cc32210913c20e36eaa5fcd0ef1/package.json)。

## 2. 直接决定代码的事实

| 源码事实 | 应用代码 |
|---|---|
| render同步返回svg/width/height；输入变化后解析累计全文 | renderDiagramSync为普通函数；删除旧pending/Subscriber/drain |
| 未闭合节点、边、subgraph会抛语法错误 | Entry分离latestSource与lastGood；只在成功时替换SVG |
| render接受idPrefix | 模板用固定prefix，插入前调用现有instantiateMermaidSvg分配实例ID |
| SVG根style包含内部变量和字体 | decorateMermaid只写尺寸属性，保留style |
| 颜色使用CSS变量，字体/宽度参与布局 | 颜色不进key；fontEpoch和maxWidth进key；导出读取computed style |
| 图内公式由同步math回调提供 | DiagramLayout携math及mathEpoch，模板key随resolver变化 |

对应固定源码：[core.ts](https://github.com/obsidianmd/pele/blob/e7f092a4de147cc32210913c20e36eaa5fcd0ef1/src/core.ts#L76-L160)、[RenderOptions](https://github.com/obsidianmd/pele/blob/e7f092a4de147cc32210913c20e36eaa5fcd0ef1/src/types.ts)、[SVG主题](https://github.com/obsidianmd/pele/blob/e7f092a4de147cc32210913c20e36eaa5fcd0ef1/src/svg/theme.ts#L3-L41)。

半截源码的执行输入和结果保存在[host-probe-1.9.0.json](evidence/host-probe-1.9.0.json)。例如A-->B[失败、补齐标签后成功，直接对应controller的错误恢复用例。
