---
title: 附录B - HTTP端点完整参考
scope: 双端
---

# 附录B - HTTP 端点完整参考

> 所有路径均挂载于 `/api/mobile-sync`。HTTP 只接受 `Authorization: Bearer <token>`；WebSocket 的 query token 是另一条连接边界。

---

## 表1：实体端点

| 路径 | 方法 | 请求 | 响应 | 限制 |
|---|---|---|---|---|
| `/entities/pull` | POST | `{items: EntitySelector[]}` | `{results: EntityPullResult[]}` | 1000 项 / 10 MiB |
| `/entities/push` | POST | `{items: EntityPushItem[]}` | `{results: EntityPushResult[]}` | 10000 项 / 10 MiB |

Owner 与 Topic 共用外壳，但 DTO 本身不合并：

```json
{"entityType":"owner","ownerType":"agent","ownerId":"agent-a"}
{"entityType":"topic","ownerType":"group","ownerId":"group-a","topicId":"topic-a"}
```

Push 项在身份字段外增加 `data`。逐项结果回显完整身份；成功为 `ok:true,data?`，失败为 `ok:false,error`。桌面内部仍让 Owner 走单配置写入，让 Topic 按父 `config.json` 分组提交。`x-idempotency-key` 只用于已有的 Owner 幂等提交，不承担认证。

---

## 表2：消息端点

| 路径 | 方法 | 请求 | 响应 | 限制 |
|---|---|---|---|---|
| `/messages/pull` | POST | `{topics:[{ownerType,ownerId,topicId,messageIds}]}` | Topic NDJSON | 10000 Topic / 100000 Message ID / 256 MiB |
| `/messages/push` | POST | Topic NDJSON | Topic NDJSON | 每 Topic 10000 状态 / 单行 32 MiB / 总量 256 MiB |

`messageIds: []` 表示拉取该 Topic 的全部 live 消息。Pull 成功行：

```ndjson
{"kind":"topic","ownerType":"agent","ownerId":"agent-a","topicId":"topic-a","ok":true,"messages":[]}
```

Push 行同时提交 live 最终视图和离线墓碑：

```ndjson
{"kind":"topic","ownerType":"agent","ownerId":"agent-a","topicId":"topic-a","messages":[],"deletedMessages":[{"msgId":"msg-a","deletedAt":1700000000000}]}
```

Push 成功响应只回显 Topic 身份和 `ok:true`。Topic 失败使用 `ok:false,error`；流级失败使用 `kind:"streamError",error`。附件只随 Message DTO 同步元数据与内容 Hash，二进制 CAS 始终留在各端本机。

---

## 表3：头像端点

| 路径 | 方法 | 身份 | Body / 响应 |
|---|---|---|---|
| `/avatars/pull` | GET | `?ownerType=&ownerId=` | 返回原始图片字节与真实 MIME |
| `/avatars/push` | POST | `?ownerType=&ownerId=` | 原始图片字节；返回 `{ownerType,ownerId,ok:true}` |

`ownerType` 为 `agent/group`，或唯一的 `user/user_avatar`。Avatar 使用独立二进制 Hash 和更新时间，不并入 Owner 双 Hash。Pull 保留最多三次指数退避；请求身份不提供默认值。

---

## 表4：删除端点

公共 HTTP 不再提供独立删除端点：

- Owner、Topic、Avatar、Message 的在线删除使用 `SYNC_ENTITY_DELETE`，携带 `targetType` 与完整身份。
- 离线 Owner/Topic/Avatar 墓碑由后续 Manifest 重放。
- 离线 Message 墓碑随 `/messages/push` 的 `deletedMessages` 重放，并与该 Topic 的 live 最终视图共用一次确认。

---

## 附录：HTTP 状态码与错误处理

| 状态码 | 场景 | 移动端行为 |
|-------|------|----------|
| `200 OK` | 请求成功 | 正常解析响应体 |
| `400 Bad Request` | 身份、字段或预算不合法 | 协议错误，终止 attempt |
| `401 Unauthorized` | Bearer token 不匹配 | 提示用户核对同步令牌 |
| `404 Not Found` | 实体或头像不存在 | 当前操作失败 |
| `409 Conflict` | 已提交状态与请求冲突 | 当前操作失败 |
| `500 Internal Server Error` | 桌面存储或 CDS 故障 | 保留结构化根因并终止 attempt |

所有非 2xx 响应使用 `{"error": WireSyncError}`。逐项失败使用 `ok:false,error`；成功禁止携带 `error`。Wire 1.5 的错误对象固定包含 `code/origin/stage/kind/retry/message/failedTopicIds`。

### 流式端点特殊错误帧

对于 `/messages/pull` 和 `/messages/push`：

- 流级错误：`{"kind":"streamError","error":WireSyncError}`。
- Topic 级错误：`{"kind":"topic",完整身份,"ok":false,"error":WireSyncError}`。

### 并发与限流

| 层级 | 并发控制 |
|------|---------|
| 消息 Pull 并发 | 32 MiB 在途帧预算，每帧按 1 MiB 单位向上取整占用 |
| 实体分块大小 | Agent/Group: 50/批；Topic: 1000/批 |
| 消息分块大小 | `MAX_MESSAGES_PER_BATCH = 10000`（控制 WS payload，非 HTTP） |

---

## 附录：端到端调用链速查

```
Phase 1 (Owner Metadata)
  PULL/PUSH Owner → /entities/pull|push
  PULL/PUSH Avatar → /avatars/pull|push

Phase 2 (Topic Metadata)
  PULL/PUSH Topic → /entities/pull|push

Phase 3 (Messages)
  PULL/PUSH Message → /messages/pull|push
```

---

*权威实现：Mobile `pull_executor.rs` / `push_executor.rs`，Desktop `transport/routes.js`。*
