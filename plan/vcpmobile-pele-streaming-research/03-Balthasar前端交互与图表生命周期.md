# 同帧交接与图表消费端：执行顺序

完整伪代码见[00第3节](00-综合结论与改造路线.md)。

## 1. promotion不依赖两个post watcher的先后

~~~text
MessageRenderer pre
  capturePromotionTemplates()
  ↓
Vue patch
  删除tail DOM / stable v-html生成新宿主
  ↓
tail post（可先执行）
  syncRoot + sweep删除旧Entry
  ↓
stable post
  prepareStyles
  completedLRU → promotionTemplates → sync render
  写SVG
  releasePromotionTemplates
  ↓
本轮浏览器绘制
~~~

capture必须放在MessageRenderer现有blocks pre watcher的动画条件return之前。promotionTemplates是lastGood对象引用数组，不受tail sweep删除Map条目的影响。新stable source与layout必须精确匹配；不命中时当场同步render。

本地Vue3.5.44源码：runtime-core的queueFlush用Promise.then(flushJobs)，flushJobs在finally同步调用flushPostFlushCbs；post callbacks在其中逐个同步调用。实际位置可用：

~~~powershell
$vueRuntime = node -e "const {createRequire}=require('node:module'); const {dirname,join}=require('node:path'); const r=createRequire(require.resolve('vue')); console.log(join(dirname(r.resolve('@vue/runtime-core/package.json')),'dist/runtime-core.cjs.js'))"
rg -n "function queueFlush|flushPostFlushCbs|pendingPostFlushCbs" $vueRuntime
~~~

contract-probe使用实际Vue调度，故意先sweep旧Entry再hydrate stable：精确命中时新增render调用0次；source增加末尾换行时新增render调用1次，两条路径nextTick完成时都已有SVG。

## 2. 样式必须早于测量

当前MessageRenderer注入CSS的watcher在:943，图表post在:684。把注入主体提为syncMessageStylesNow，controller每次syncRoot/refreshLayout之前直接调用；不要靠注册顺序。

layout从host.clientWidth扣除host与wrapper的padding、wrapper边框，fontFamily/fontSize读取wrapper计算样式；隐藏viewport不参与宽度测量。同一controller共享now、fontEpoch/mathEpoch。

## 3. 消费函数前后对照

| 函数 | 删除 | 新执行 |
|---|---|---|
| renderHeavyContent | Mermaid循环；主题变化时source覆盖SVG | 数学与表情保留为renderMathAndEmoticons |
| decorateMermaid | removeAttribute('style')；清空host | 保持wrapper/button；只替换SVG节点、写尺寸 |
| useMessageEvents图表点击 | stable-only门槛；取latestSource | controller.getRendered，读取lastGood配对源码 |
| Viewer.copySvg | 直接复制props.svgHtml | 序列化当前connected SVG的computed样式 |
| exportSvg | 排队后才读取源DOM | 入队前拍摄尺寸和样式，队列内继续现有Image/Canvas编码 |

renderedImageExport的样式函数只改clone；同文档marker URL本地化为url(#id)，外部SVG URL保持原值。SVG根的字体、Pele变量和图内显式颜色继续保留。
