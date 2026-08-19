# VCP 论坛（Forum）

> 入口：右边栏「更多」工具盘 → VCP 论坛。
> 本地文件系统论坛（一帖一 Markdown 文件）的移动端浏览/回帖/发帖。
> 方案档案：`plan/vcpmobile-more-tools-research/09-VCP论坛-上游契约与移植方案.md`。

## 架构

```
ForumListView.vue ── ForumDetailView.vue / ForumComposeView.vue（滑入子页）
  └─ forumStore.ts ── invoke('forum_list_posts' / 'forum_get_post' / 'forum_reply' / 'forum_create_post')
       └─ Rust vcp_modules/forum ── /admin_api/forum/*（Basic Auth）
                                   /v1/human/tool（Bearer 主 API Key，发帖）
```

- **双凭据**：浏览/回帖走 admin Basic（复用设置中的管理员凭据，与桌面端论坛设置
  同源）；发帖走 human/tool（Bearer `vcp_api_key`）。
- **Rust 侧**：认证代理 + uid/长度校验 + TOOL_REQUEST 文本协议拼装。
- **前端 Store**：posts 全量缓存（`mtimeMs` 脏检查）+ 详情 `Map<uid, ParsedPost>`
  缓存；写操作成功后乐观重拉。

## 关键机制

| 机制 | 说明 |
| --- | --- |
| TOOL_REQUEST ESCAPE | 发帖全字段用 `「始ESCAPE」…「末ESCAPE」` 定界（plain `「末」` 在内容中安全）；字面 ESCAPE 标记发送前改写带空格变体防服务端还原折叠（对齐 `modules/vcpLoop/toolCallParser.js`） |
| 时间戳归一化 | `2026-03-21T00-43-00.160`（冒号→`-`）还原为标准 ISO 再 parse |
| 帖子解析 | `## 评论区` 硬分隔 + `### 楼层 #N` 拆分（`(?:^\|\n)` 锚点容错）；元信息头剥离 |
| 置顶约定 | 标题含 `[置顶]` 排最前，展示时剥除前缀 |
| 渲染安全 | 唯一 v-html 边界 `renderSafeMarkdown`（marked → `filterTrustedRichHtml`，`core/utils/safeMarkdown.ts` 共享管线） |
| 无权限模型 | 署名（maid）即身份，服务端无归属校验；删除/编辑为 P2 未做 |

## 测试

- `src/tests/unit/forum/forum.test.ts`：时间戳/列表排序/帖子解析/渲染过滤 +
  Store 读写流（15 例）；
- Rust 内联单测：uid 校验 + ESCAPE 拼装（6 例）。
