# VCPMobile Distributed Synchronization Architecture

> **归档文档**  
> 本文档是 v0.9.13 之前的原始设计规范，内容已迁移并维护于 `docs/sync/` 目录下的同步 V2 知识库。  
> 请勿以本文档作为当前实现的权威参考。

> **📌 文档迁移提示**：本文档是同步模块的原始设计规范（v0.9.13 之前）。自 2026-05-13 起，同步模块 V2 的详细知识库已迁移至 `docs/sync/` 目录，包含 20 份与代码深度绑定的工程文档（总计 13000+ 行），涵盖架构、协议、实现、对照手册和开发指南。建议优先查阅新文档，本文档保留作为历史参考。
>
> **快速入口**：
> - 架构总览 → `docs/sync/00_总览与导航.md`
> - 协议详解 → `docs/sync/04_同步协议详解.md`
> - 双端对照 → `docs/sync/14_双端精确对照手册.md`
> - 开发指南 → `docs/sync/15_开发指南与FAQ.md`

## Table of Contents
1. [System Overview](#system-overview)
2. [Core Data Structures](#core-data-structures)
3. [Synchronization Protocol](#synchronization-protocol)
4. [Multi-Phase Processing](#multi-phase-processing)
5. [Conflict Resolution](#conflict-resolution)
6. [Hash-Based Change Detection](#hash-based-change-detection)
7. [Communication Protocols](#communication-protocols)
8. [Error Handling and Reliability](#error-handling-and-reliability)
9. [Performance Optimization](#performance-optimization)
10. [Offline Support](#offline-support)

---

## System Overview

VCPMobile implements a distributed synchronization system designed for mobile-desktop collaboration. The architecture follows a **LWW (Last-Write-Wins) + Hash Arbitration** conflict resolution strategy with a three-phase synchronization protocol.

### Architecture Components

```
┌─────────────────────────────────────────────────────────────┐
│                     SyncService (Orchestrator)              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  WebSocket Connection (Real-time notifications)      │  │
│  │  HTTP Client (Bulk data transfer)                    │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Phase 1   │→│   Phase 2   │→│   Phase 3   │         │
│  │  Metadata   │  │   Topics    │  │  Messages  │         │
│  └─────────────┘  └─────────────┘  └─────────────┘        │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  PullExecutor │ PushExecutor │ DeleteExecutor        │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  HashAggregator │ HashInitializer │ Phase1Metadata   │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Key Design Principles

1. **Deterministic Hashing**: All data structures use SHA-256 hashes with canonical JSON serialization
2. **Merkle Tree Aggregation**: Hierarchical hash aggregation for efficient change detection
3. **Idempotent Operations**: All sync operations are idempotent with unique keys
4. **Soft Deletion**: Bidirectional deletion synchronization with timestamps
5. **Network-Aware Concurrency**: Adaptive semaphore based on network conditions

---

## Core Data Structures

### EntityState - State Vector

```rust
pub struct EntityState {
    pub id: String,              // Unique entity identifier
    pub hash: String,            // SHA-256 content fingerprint
    pub ts: i64,                 // Absolute timestamp (LWW arbiter)
    pub deleted_at: Option<i64>, // Soft deletion timestamp
    pub owner_type: Option<String>, // For topics: "agent" or "group"
}
```

**Purpose**: Represents the complete state of an entity for manifest exchange.

**Hash Calculation**:
- Agent: `SHA256(config_hash || content_hash)`
- Group: `SHA256(config_hash || content_hash)`
- Topic: `SHA256(metadata_hash || content_hash)`
- Avatar: Direct SHA256 of binary data

### SyncManifest

```rust
pub struct SyncManifest {
    pub data_type: SyncDataType,  // Agent, Group, Avatar, Topic, Message
    pub items: Vec<EntityState>,  // Collection of entity states
}
```

**Transmission Protocol**:
```json
{
  "type": "SYNC_MANIFEST",
  "dataType": "agent",
  "phase": "metadata",
  "data": [
    {
      "id": "agent_001",
      "hash": "a3f2b8c9...",
      "ts": 1712345678901,
      "deletedAt": null
    }
  ]
}
```

### SyncDataType Enumeration

```rust
pub enum SyncDataType {
    Agent,   // AI agent configurations
    Group,   // Multi-agent groups
    Avatar,  // Profile images
    Topic,   // Conversation threads
    Message, // Individual messages
}
```

### Data Transfer Objects (DTOs)

#### AgentSyncDTO
```rust
pub struct AgentSyncDTO {
    pub name: String,
    pub system_prompt: String,
    pub model: String,
    pub temperature: f32,
    pub context_token_limit: i32,
    pub max_output_tokens: i32,
    pub stream_output: bool,
}
```

#### GroupSyncDTO
```rust
pub struct GroupSyncDTO {
    pub name: String,
    pub members: Vec<String>,
    pub mode: String,
    pub member_tags: Option<serde_json::Value>,
    pub group_prompt: Option<String>,
    pub invite_prompt: Option<String>,
    pub use_unified_model: bool,
    pub unified_model: Option<String>,
    pub tag_match_mode: Option<String>,
    pub created_at: i64,
}
```

#### AgentTopicSyncDTO (includes UI state)
```rust
pub struct AgentTopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub locked: bool,    // UI state: conversation locked
    pub unread: bool,    // UI state: unread indicator
    pub owner_id: String,
}
```

#### GroupTopicSyncDTO (no UI state)
```rust
pub struct GroupTopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub owner_id: String,
}
```

---

## Synchronization Protocol

### Connection Establishment

**Step 1: WebSocket Connection**
```
Client → Server: WebSocket handshake with token
ws://server:port?token=<sync_token>

Server → Client: Connection acknowledgment
Status: "connected"
```

**Step 2: Hash Initialization**
```rust
// Ensure all agents have computed hashes
HashInitializer::ensure_all_agent_hashes(&pool).await?;

// Ensure all groups have computed hashes
HashInitializer::ensure_all_group_hashes(&pool).await?;
```

**Purpose**: Guarantees all local entities have valid hashes before manifest exchange.

### Three-Phase Synchronization

#### Phase 1: Metadata Synchronization

**Objective**: Synchronize agents, groups, and avatars.

**Protocol Flow**:
```
1. Client builds manifests:
   - Agent manifest (all agents with aggregated hashes)
   - Group manifest (all groups with aggregated hashes)
   - Avatar manifest (all avatars with binary hashes)

2. Client → Server: SYNC_MANIFEST (agent)
   Client → Server: SYNC_MANIFEST (group)
   Client → Server: SYNC_MANIFEST (avatar)

3. Server computes diff and responds:
   Server → Client: SYNC_DIFF_RESULTS
   {
     "dataType": "agent",
     "data": [
       {"id": "agent_001", "action": "PULL"},
       {"id": "agent_002", "action": "PUSH"},
       {"id": "agent_003", "action": "DELETE", "deletedAt": 1712345678},
       {"id": "agent_004", "action": "PUSH_DELETE", "deletedAt": 1712345680}
     ]
   }

4. Client executes actions in parallel:
   - PULL: HTTP GET /api/mobile-sync/download-entity
   - PUSH: HTTP POST /api/mobile-sync/upload-entity
   - DELETE: Local soft deletion
   - PUSH_DELETE: Notify server of local deletion

5. On completion: Trigger Phase 2
```

**Diff Computation Algorithm**:
```rust
for each remote entity:
    if local exists:
        if local.hash == remote.hash:
            action = SKIP
        else if local.ts < remote.ts:
            action = PULL
        else if local.ts > remote.ts:
            action = PUSH
        else:
            // Timestamp collision - hash arbitration
            action = if local.hash < remote.hash { PULL } else { PUSH }
    else:
        action = PULL

for each local entity not in remote:
    action = PUSH
```

#### Phase 2: Topic Synchronization

**Objective**: Synchronize conversation topics (agent topics and group topics).

**Protocol Flow**:
```
1. Client → Server: PHASE_START {"phase": "topic"}

2. Server → Client: PHASE_MANIFESTS
   {
     "manifests": [
       {
         "dataType": "topic",
         "items": [
           {
             "id": "topic_001",
             "ownerType": "agent",
             "hash": "d4e5f6...",
             "ts": 1712345690
           }
         ]
       }
     ]
   }

3. Client computes topic diff:
   - Build local topic manifest with metadata hashes
   - Compare with remote manifest
   - Determine PULL/PUSH actions per topic

4. Execute topic synchronization:
   - PULL agent_topic: HTTP GET /api/mobile-sync/download-entity?type=agent_topic
   - PULL group_topic: HTTP GET /api/mobile-sync/download-entity?type=group_topic
   - PUSH agent_topic: HTTP POST with AgentTopicSyncDTO
   - PUSH group_topic: HTTP POST with GroupTopicSyncDTO

5. On completion: Trigger Phase 3
```

**Topic Hash Calculation**:
```rust
// Agent topic
metadata_hash = compute_agent_topic_metadata_hash(dto);
aggregate_hash = SHA256(metadata_hash || content_hash);

// Group topic
metadata_hash = compute_group_topic_metadata_hash(dto);
aggregate_hash = SHA256(metadata_hash || content_hash);
```

#### Phase 3: Message Synchronization

**Objective**: Synchronize messages for all active topics.

**Protocol Flow**:
```
1. Client → Server: PHASE_START {"phase": "message"}

2. For each active topic:
   Client → Server: GET_MESSAGE_MANIFEST {"topicId": "topic_001"}

3. Server responds for each topic:
   Server → Client: MESSAGE_MANIFEST_RESULTS
   {
     "topicId": "topic_001",
     "messages": [
       {
         "msgId": "msg_001",
         "contentHash": "abc123...",
         "updatedAt": 1712345700
       }
     ]
   }

4. Client computes message diff per topic:
   - Load local message hashes from database
   - Compare with remote message manifest
   - Determine messages to PULL and if PUSH is needed

5. Execute message synchronization:
   - PULL: HTTP POST /api/mobile-sync/download-messages
     Body: {"topicId": "...", "msgIds": [...]}
   
   - PUSH: HTTP POST /api/mobile-sync/upload-messages
     Body: {"topicId": "...", "messages": [...]}
     
   - If server needs attachments:
     Server responds with: {"neededAttachmentHashes": [...]}
     Client uploads each attachment:
       HTTP POST /api/mobile-sync/upload-attachment?hash=...&type=...

6. Update topic and parent hashes:
   bubble_topic_hash(topic_id)
   bubble_agent_hash(agent_id) OR bubble_group_hash(group_id)

7. On all topics complete: Send PHASE_COMPLETED
```

**Message Fingerprint Calculation**:
```rust
fn compute_message_fingerprint(content: &str, attachment_hashes: &[String]) -> String {
    let mut sorted_hashes = attachment_hashes.to_vec();
    sorted_hashes.sort();
    
    let fingerprint = json!({
        "content": content,
        "attachmentHashes": sorted_hashes
    });
    
    compute_deterministic_hash(&fingerprint)
}
```

---

## Multi-Phase Processing

### Pipeline State Machine

```rust
pub enum PipelinePhase {
    Idle,
    Phase1Metadata { progress: PhaseProgress },
    Phase2Topic { progress: PhaseProgress },
    Phase3Message { progress: PhaseProgress },
    Completed,
    Failed { error: String, phase: String },
}
```

### Phase Progress Tracking

```rust
pub struct PhaseProgress {
    pub total: u32,
    pub completed: u32,
    pub pending: Vec<String>,
}
```

### Pipeline Execution Flow

```
┌──────┐   Phase 1    ┌──────────┐   Phase 2    ┌──────────┐   Phase 3    ┌───────────┐
│ Idle │ ──────────→ │ Metadata │ ──────────→ │  Topics  │ ──────────→ │ Messages  │
└──────┘             └──────────┘             └──────────┘             └───────────┘
                            │                        │                        │
                            ↓                        ↓                        ↓
                      Hash Init              Topic Manifest          Message Manifest
                            │                        │                        │
                            ↓                        ↓                        ↓
                      Send Manifests           Compute Diff            Per-Topic Diff
                            │                        │                        │
                            ↓                        ↓                        ↓
                      Receive Diff            Execute Pull/Push       Execute Pull/Push
                            │                        │                        │
                            ↓                        ↓                        ↓
                      Execute Actions         Bubble Hashes           Bubble Hashes
                            │                        │                        │
                            └────────────────────────┴────────────────────────┘
                                                     ↓
                                              ┌───────────┐
                                              │ Completed │
                                              └───────────┘
```

### Phase Transition Triggers

```rust
// Phase 1 → Phase 2
on_phase1_completed(pool):
    ensure_all_agent_hashes(pool)
    ensure_all_group_hashes(pool)
    state = Phase2Topic { progress: PhaseProgress::new() }
    send(PipelineCommand::Phase1)

// Phase 2 → Phase 3
on_phase2_completed():
    state = Phase3Message { progress: PhaseProgress::new() }
    send(PipelineCommand::Phase2)

// Phase 3 → Completed
on_phase3_completed():
    state = Completed
    send(PipelineCommand::Phase3)
```

---

## Conflict Resolution

### LWW (Last-Write-Wins) Strategy

**Core Principle**: The entity with the later timestamp wins.

```rust
pub enum DiffResult {
    Skip,   // Hashes match - no action needed
    Pull,   // Remote is newer (remote.ts > local.ts)
    Push,   // Local is newer (local.ts > remote.ts)
    Arbitrated { action: ArbitratedAction }, // Timestamp collision
}
```

### Diff Computation Algorithm

```rust
fn compute(local: &EntityState, remote: &EntityState) -> DiffResult {
    // Step 1: Check if content is identical
    if local.hash == remote.hash {
        return DiffResult::Skip;
    }
    
    // Step 2: Compare timestamps
    if local.ts < remote.ts {
        DiffResult::Pull  // Remote is newer
    } else if local.ts > remote.ts {
        DiffResult::Push  // Local is newer
    } else {
        // Step 3: Timestamp collision - deterministic arbitration
        if local.hash < remote.hash {
            DiffResult::Arbitrated { action: ArbitratedAction::Pull }
        } else {
            DiffResult::Arbitrated { action: ArbitratedAction::Push }
        }
    }
}
```

### Hash Arbitration for Timestamp Collisions

**Scenario**: Two clients modify the same entity at the exact same millisecond.

**Resolution**: Use hash string comparison as a tiebreaker.

**Guarantee**: Both clients will make the **same decision** because:
1. Hash comparison is deterministic
2. Both clients use the same comparison direction (`local.hash < remote.hash`)

**Example**:
```
Local:  { hash: "a3f2b8c9...", ts: 1712345678901 }
Remote: { hash: "d7e9f1a2...", ts: 1712345678901 }

Comparison: "a3f2b8c9..." < "d7e9f1a2..." → true
Result: Both clients choose PULL (remote wins)
```

### Deletion Synchronization

**Soft Deletion Model**: Entities are marked with `deleted_at` timestamp instead of hard deletion.

**Deletion Actions**:
```rust
pub enum DiffAction {
    Pull,
    Push,
    Delete { deleted_at: i64 },      // Remote deleted, apply locally
    PushDelete { deleted_at: i64 },  // Local deleted, notify remote
    Skip,
}
```

**Deletion Diff Algorithm**:
```rust
fn compute_with_deletion(
    local_items: &[EntityState],
    remote_items: &[EntityState],
    local_deleted: &HashMap<String, i64>,
    remote_deleted: &HashMap<String, i64>,
) -> ManifestDiff {
    for remote in remote_items {
        // Check if remote has deleted this entity
        if let Some(deleted_at) = remote_deleted.get(&remote.id) {
            if !local_deleted.contains_key(&remote.id) {
                // Remote deleted, local hasn't - apply deletion
                actions.push((remote.id, Delete { deleted_at }));
            }
            continue;
        }
        
        // Check if local has deleted this entity
        if let Some(deleted_at) = local_deleted.get(&remote.id) {
            // Local deleted, remote hasn't - notify remote
            actions.push((remote.id, PushDelete { deleted_at }));
            continue;
        }
        
        // Normal diff computation for non-deleted entities
        // ...
    }
}
```

---

## Hash-Based Change Detection

### Deterministic Hash Computation

**Canonical JSON Serialization**:
```rust
fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();  // Sort keys for determinism
            
            let mut res = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 { res.push(','); }
                res.push_str(&format!("\"{}\":{}", k, stable_stringify(map.get(k))));
            }
            res.push('}');
            res
        }
        Value::Array(arr) => {
            // Arrays maintain order
            let mut res = String::from("[");
            for (i, v) in arr.iter().enumerate() {
                if i > 0 { res.push(','); }
                res.push_str(&stable_stringify(v));
            }
            res.push(']');
            res
        }
        // Primitives...
    }
}
```

**SHA-256 Hash**:
```rust
fn compute_deterministic_hash<T: Serialize>(data: &T) -> String {
    let val = serde_json::to_value(data);
    let json_str = stable_stringify(&val);
    
    let mut hasher = Sha256::new();
    hasher.update(json_str.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

### Merkle Tree Aggregation

**Purpose**: Efficiently detect changes in hierarchical structures.

**Agent Hash Aggregation**:
```
Agent Root Hash
    │
    ├─ config_hash (agent configuration)
    │
    └─ content_hash (Merkle root of topics)
           │
           ├─ topic_001 content_hash
           ├─ topic_002 content_hash
           └─ topic_003 content_hash
```

**Implementation**:
```rust
async fn compute_agent_root_hash(tx: &mut Transaction, agent_id: &str) -> String {
    // Get config hash
    let config_hash = query("SELECT config_hash FROM agents WHERE agent_id = ?");
    
    // Get all topic content hashes
    let topic_rows = query(
        "SELECT content_hash FROM topics 
         WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL 
         ORDER BY topic_id ASC"
    );
    
    // Compute Merkle root
    let mut hashes = vec![config_hash];
    for row in topic_rows {
        hashes.push(row.get("content_hash"));
    }
    
    compute_merkle_root(hashes)
}

fn compute_merkle_root(mut hashes: Vec<String>) -> String {
    hashes.sort();  // Deterministic ordering
    
    let mut hasher = Sha256::new();
    for h in hashes {
        hasher.update(h.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}
```

### Hash Bubbling

**Purpose**: Propagate changes up the hierarchy after modifications.

**Flow**:
```
Message Update
    ↓
Update message content_hash
    ↓
Bubble topic hash (recompute from all messages)
    ↓
Bubble agent/group hash (recompute from config + all topics)
```

**Implementation**:
```rust
async fn bubble_from_topic(tx: &mut Transaction, topic_id: &str) {
    // Step 1: Update topic hash
    bubble_topic_hash(tx, topic_id).await;
    
    // Step 2: Get topic owner
    let (owner_id, owner_type) = query(
        "SELECT owner_id, owner_type FROM topics WHERE topic_id = ?"
    );
    
    // Step 3: Update parent hash
    if owner_type == "agent" {
        bubble_agent_hash(tx, &owner_id).await;
    } else {
        bubble_group_hash(tx, &owner_id).await;
    }
}
```

### Hash Initialization

**Purpose**: Ensure all entities have valid hashes before sync.

**Scenario**: Legacy entities created before hash system was implemented.

```rust
async fn ensure_agent_hashes(tx: &mut Transaction, agent_id: &str) {
    let config_hash = query("SELECT config_hash FROM agents WHERE agent_id = ?");
    
    if config_hash.is_empty() || config_hash == "PENDING" {
        // Load agent data and compute hash
        let dto = load_agent_dto(tx, agent_id).await;
        let new_hash = compute_agent_config_hash(&dto);
        
        // Update database
        query("UPDATE agents SET config_hash = ? WHERE agent_id = ?");
    }
}
```

---

## Communication Protocols

### WebSocket Protocol (Real-time Notifications)

**Connection**:
```rust
let ws_url = format!("ws://{}?token={}", server_url, sync_token);
let (ws_stream, _) = connect_async(&ws_url).await;
```

**Message Types**:

#### Client → Server Messages

**1. SYNC_MANIFEST** (Phase 1)
```json
{
  "type": "SYNC_MANIFEST",
  "dataType": "agent",
  "phase": "metadata",
  "data": [
    {"id": "agent_001", "hash": "abc123", "ts": 1712345678}
  ]
}
```

**2. PHASE_START** (Phase 2/3)
```json
{
  "type": "PHASE_START",
  "phase": "topic"
}
```

**3. GET_MESSAGE_MANIFEST** (Phase 3)
```json
{
  "type": "GET_MESSAGE_MANIFEST",
  "topicId": "topic_001"
}
```

**4. SYNC_ENTITY_UPDATE** (Real-time notification)
```json
{
  "type": "SYNC_ENTITY_UPDATE",
  "id": "agent_001",
  "dataType": "agent",
  "hash": "def456",
  "ts": 1712345690
}
```

**5. SYNC_DELETE_NOTIFY**
```json
{
  "type": "SYNC_DELETE_NOTIFY",
  "id": "agent_001",
  "dataType": "agent"
}
```

**6. PHASE_COMPLETED**
```json
{
  "type": "PHASE_COMPLETED"
}
```

#### Server → Client Messages

**1. SYNC_DIFF_RESULTS** (Phase 1 response)
```json
{
  "type": "SYNC_DIFF_RESULTS",
  "dataType": "agent",
  "data": [
    {"id": "agent_001", "action": "PULL"},
    {"id": "agent_002", "action": "PUSH"}
  ]
}
```

**2. PHASE_MANIFESTS** (Phase 2 response)
```json
{
  "type": "PHASE_MANIFESTS",
  "manifests": [
    {
      "dataType": "topic",
      "items": [...]
    }
  ]
}
```

**3. MESSAGE_MANIFEST_RESULTS** (Phase 3 response)
```json
{
  "type": "MESSAGE_MANIFEST_RESULTS",
  "topicId": "topic_001",
  "messages": [
    {
      "msgId": "msg_001",
      "contentHash": "abc123",
      "updatedAt": 1712345700
    }
  ]
}
```

**4. SYNC_ENTITY_UPDATE** (Real-time notification from server)
```json
{
  "type": "SYNC_ENTITY_UPDATE",
  "id": "topic_001",
  "dataType": "topic",
  "ownerType": "agent"
}
```

**5. SYNC_DELETE_NOTIFY** (Deletion notification)
```json
{
  "type": "SYNC_DELETE_NOTIFY",
  "id": "agent_001",
  "dataType": "agent"
}
```

### HTTP Protocol (Bulk Data Transfer)

#### Pull Operations

**1. Download Entity**
```
GET /api/mobile-sync/download-entity?id={id}&type={type}
Header: x-sync-token: {token}

Response:
- Agent: AgentSyncDTO (JSON)
- Group: GroupSyncDTO (JSON)
- Agent Topic: AgentTopicSyncDTO (JSON)
- Group Topic: GroupTopicSyncDTO (JSON)
```

**2. Download Avatar**
```
GET /api/mobile-sync/download-avatar?id={owner_id}&type={owner_type}
Header: x-sync-token: {token}

Response: Binary image data
Content-Type: image/png or image/jpeg
```

**3. Download Messages**
```
POST /api/mobile-sync/download-messages
Header: x-sync-token: {token}
Body: {"topicId": "...", "msgIds": ["msg_001", "msg_002"]}

Response: Array of message objects
[
  {
    "id": "msg_001",
    "role": "user",
    "content": "...",
    "timestamp": 1712345700,
    "attachments": [...]
  }
]
```

#### Push Operations

**1. Upload Entity**
```
POST /api/mobile-sync/upload-entity
Headers:
  x-sync-token: {token}
  x-idempotency-key: {SHA256(action + type + id + minute_timestamp)}

Body:
{
  "id": "agent_001",
  "type": "agent",
  "data": { ... AgentSyncDTO ... }
}
```

**Idempotency Key Generation**:
```rust
fn generate_idempotency_key(action: &str, entity_type: &str, id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action.as_bytes());
    hasher.update(entity_type.as_bytes());
    hasher.update(id.as_bytes());
    
    // Bucket by minute to allow retries within the same minute
    let minute = chrono::Utc::now().timestamp() / 60;
    hasher.update(minute.to_string().as_bytes());
    
    format!("{:x}", hasher.finalize())
}
```

**2. Upload Avatar**
```
POST /api/mobile-sync/upload-avatar?id={owner_id}&type={owner_type}
Headers:
  x-sync-token: {token}
  Content-Type: {mime_type}

Body: Binary image data
```

**3. Upload Messages**
```
POST /api/mobile-sync/upload-messages
Header: x-sync-token: {token}
Body:
{
  "topicId": "topic_001",
  "messages": [
    {
      "id": "msg_001",
      "role": "user",
      "content": "...",
      "timestamp": 1712345700,
      "attachments": [...]
    }
  ]
}

Response:
{
  "neededAttachmentHashes": ["hash1", "hash2"]
}
```

**4. Upload Attachment**
```
POST /api/mobile-sync/upload-attachment?hash={hash}&type={mime_type}
Headers:
  x-sync-token: {token}
  Content-Type: application/octet-stream

Body: Binary file data
```

---

## Error Handling and Reliability

### Retry Mechanisms

#### Database Lock Retry

```rust
pub struct RetryConfig {
    pub max_retries: u32,      // Default: 3
    pub base_delay_ms: u64,    // Default: 100ms
    pub max_delay_ms: u64,     // Default: 2000ms
}

async fn retry_on_db_locked<F, T>(
    config: &RetryConfig,
    operation: F,
    operation_name: &str,
) -> Result<T, String> {
    let mut delay = config.base_delay_ms;
    
    for attempt in 0..config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if e.contains("database is locked") => {
                sleep(Duration::from_millis(delay)).await;
                delay = (delay * 2).min(config.max_delay_ms);
            }
            Err(e) => return Err(e),
        }
    }
    
    Err(format!("{} failed after {} retries", operation_name, config.max_retries))
}
```

**Exponential Backoff**: 100ms → 200ms → 400ms → 800ms → 1600ms → 2000ms (capped)

#### Network Retry Policy

```rust
pub struct RetryPolicy {
    pub max_retries: u32,       // Default: 3
    pub base_delay_ms: u64,     // Default: 200ms
    pub max_delay_ms: u64,      // Default: 5000ms
    pub jitter_factor: f64,     // Default: 0.1 (10%)
}

fn is_network_retryable(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("timeout") ||
    e.contains("connection reset") ||
    e.contains("connection refused") ||
    e.contains("502") ||
    e.contains("503") ||
    e.contains("504") ||
    e.contains("429")
}
```

**Jitter Calculation**:
```rust
fn calculate_delay(&self, attempt: u32) -> Duration {
    let exponential_delay = self.base_delay_ms * 2u64.pow(attempt);
    let capped_delay = exponential_delay.min(self.max_delay_ms);
    
    let jitter_range = (capped_delay as f64 * self.jitter_factor) as u64;
    let jitter = rand::thread_rng().gen_range(0..=jitter_range);
    
    Duration::from_millis(capped_delay + jitter)
}
```

### Sync Logger

**Purpose**: Comprehensive logging with phase tracking and error aggregation.

```rust
pub struct SyncLogger {
    session_id: String,
    log_level: LogLevel,
    phases: HashMap<String, Arc<SyncPhaseMetrics>>,
    error_aggregator: ErrorAggregator,
}

pub struct SyncPhaseMetrics {
    pub phase_name: String,
    pub started_at: Instant,
    pub expected_count: AtomicU32,
    pub success_count: AtomicU32,
    pub error_count: AtomicU32,
}
```

**Session Lifecycle**:
```rust
// Start session
let logger = SyncLogger::new_session(LogLevel::Info);
// Session ID: "sync_1712345678901_a3f2b8c9"

// Start phase
logger.start_phase("metadata", expected_count);

// Log operations
logger.log_operation("metadata", "agent", "agent_001", true, Some("pulled"));
logger.log_operation("metadata", "agent", "agent_002", false, Some("database locked"));

// Complete phase
let summary = logger.complete_phase("metadata");
// Summary: { expected: 10, success: 8, errors: 2, duration_ms: 1523 }

// End session
logger.end_session();
```

**Error Aggregation**:
```rust
pub struct ErrorAggregator {
    errors: HashMap<String, Vec<ErrorDetail>>,
}

pub struct ErrorDetail {
    pub id: String,
    pub error: String,
    pub timestamp: u64,
    pub retryable: bool,
}

// Get summary
let summary = logger.get_error_summary("metadata");
// { total: 2, retryable: 1, non_retryable: 1, details: [...] }
```

### Sync Metrics

```rust
pub struct SyncMetrics {
    pub total_operations: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub retries: AtomicU64,
}
```

**Frontend Emission**:
```rust
fn emit_to_frontend(&self, app_handle: &AppHandle) {
    let metrics = json!({
        "total": self.total_operations.load(Ordering::Relaxed),
        "completed": self.completed.load(Ordering::Relaxed),
        "failed": self.failed.load(Ordering::Relaxed),
        "retries": self.retries.load(Ordering::Relaxed),
    });
    app_handle.emit("vcp-sync-metrics", metrics);
}
```

### Operation Tracker

**Purpose**: Prevent duplicate concurrent operations.

```rust
pub struct SyncOperationTracker {
    in_progress: DashMap<String, Instant>,
    ttl: Duration,  // Default: 300 seconds
}

fn try_start(&self, op_key: &str) -> bool {
    if self.in_progress.contains_key(op_key) {
        if let Some(start_time) = self.in_progress.get(op_key) {
            if Instant::now().duration_since(*start_time) < self.ttl {
                return false;  // Operation still in progress
            }
        }
    }
    self.in_progress.insert(op_key.to_string(), Instant::now());
    true
}

fn finish(&self, op_key: &str) {
    self.in_progress.remove(op_key);
}
```

---

## Performance Optimization

### Network-Aware Semaphore

**Purpose**: Adaptive concurrency based on network conditions.

```rust
pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,  // Default: 10 permits
    network_type: Arc<RwLock<NetworkType>>,
    success_streak: AtomicU32,
    failure_streak: AtomicU32,
}

pub enum NetworkType {
    WiFi,
    Cell5G,
    Cell4G,
    Unknown,
}
```

**Adaptive Behavior**:
```rust
fn on_success(&self) {
    self.success_streak.fetch_add(1, Ordering::Relaxed);
    self.failure_streak.store(0, Ordering::Relaxed);
    
    // Potential: Increase semaphore permits on sustained success
}

fn on_failure(&self) {
    self.success_streak.store(0, Ordering::Relaxed);
    self.failure_streak.fetch_add(1, Ordering::Relaxed);
    
    // Potential: Decrease semaphore permits on sustained failures
}
```

### Database Write Queue

**Purpose**: Batch database writes to reduce lock contention.

```rust
pub struct DbWriteQueue {
    pool: SqlitePool,
    tx: mpsc::UnboundedSender<DbWriteTask>,
}

pub enum DbWriteTask {
    Agent { id: String, dto: AgentSyncDTO },
    Group { id: String, dto: GroupSyncDTO },
    Avatar { owner_type: String, owner_id: String, bytes: Vec<u8> },
    AgentTopic { topic_id: String, dto: AgentTopicSyncDTO },
    GroupTopic { topic_id: String, dto: GroupTopicSyncDTO },
    Messages { topic_id: String, owner_id: String, owner_type: String, messages: Vec<ChatMessage> },
}
```

**Write Processing**:
```rust
async fn process_write_task(task: DbWriteTask, pool: &SqlitePool) {
    match task {
        DbWriteTask::Agent { id, dto } => {
            // Use retry_on_db_locked for resilience
            retry_on_db_locked(&RetryConfig::default(), || async {
                let mut tx = pool.begin().await?;
                
                // Update agent config
                query("UPDATE agents SET name = ?, system_prompt = ?, ...");
                
                // Compute and update config hash
                let config_hash = compute_agent_config_hash(&dto);
                query("UPDATE agents SET config_hash = ?");
                
                // Bubble up to content hash
                bubble_agent_hash(&mut tx, &id).await?;
                
                tx.commit().await?;
                Ok(())
            }, "write_agent").await;
        }
        // ... other task types
    }
}
```

### Parallel Execution

**Phase 1 Parallel Pull/Push**:
```rust
for item in diff_results {
    tokio::spawn(async move {
        let _permit = semaphore.acquire().await;  // Limit concurrency
        
        match action {
            "PULL" => PullExecutor::pull_agent(...).await,
            "PUSH" => PushExecutor::push_agent(...).await,
            // ...
        }
        
        // Decrement pending counter
        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            // All operations complete - trigger next phase
            tx.send(SyncCommand::Phase1);
        }
    });
}
```

**Phase 3 Per-Topic Parallelism**:
```rust
for topic_id in active_topics {
    // Each topic processed in parallel
    tokio::spawn(async move {
        // Pull messages
        PullExecutor::pull_messages(...).await;
        
        // Push messages
        PushExecutor::push_messages(...).await;
        
        // Update hashes
        bubble_from_topic(&mut tx, &topic_id).await;
        
        // Decrement counter
        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            tx.send(SyncCommand::Phase3);
        }
    });
}
```

### Attachment Deduplication

**Upload Tracking**:
```rust
pub struct SyncState {
    pub uploaded_hashes: Arc<RwLock<HashSet<String>>>,
}

async fn push_messages(..., uploaded_hashes: Option<Arc<RwLock<HashSet<String>>>>) {
    let response = client.post("/upload-messages").json(&messages).send().await;
    
    if let Some(needed_hashes) = response["neededAttachmentHashes"].as_array() {
        for hash in needed_hashes {
            let should_upload = if let Some(tracker) = &uploaded_hashes {
                !tracker.read().await.contains(hash)
            } else {
                true
            };
            
            if should_upload {
                upload_attachment(..., hash).await;
                
                if let Some(tracker) = &uploaded_hashes {
                    tracker.write().await.insert(hash.to_string());
                }
            }
        }
    }
}
```

**Benefit**: Avoids re-uploading the same attachment across multiple topics.

---

## Offline Support

### Connection Recovery

**Reconnection Loop**:
```rust
loop {
    // Step 1: Get server URLs from settings
    let (ws_url, http_url) = read_settings().await;
    
    // Step 2: Update status
    publish_sync_status("connecting").await;
    
    // Step 3: Attempt connection
    match connect_async(&ws_url).await {
        Ok((ws_stream, _)) => {
            publish_sync_status("connected").await;
            
            // Step 4: Initialize hashes
            ensure_all_agent_hashes(&pool).await;
            ensure_all_group_hashes(&pool).await;
            
            // Step 5: Start Phase 1
            let manifests = build_all_manifests(&pool).await;
            for manifest in manifests {
                ws_stream.send(Message::Text(manifest.to_string())).await;
            }
            
            // Step 6: Process messages
            loop {
                tokio::select! {
                    Some(cmd) = command_rx.recv() => { /* handle command */ }
                    Some(msg) = ws_stream.next() => { /* handle message */ }
                }
            }
        }
        Err(_) => {
            // Connection failed - wait and retry
            sleep(Duration::from_secs(10)).await;
            continue;
        }
    }
}
```

### Local Change Queueing

**Notify Local Change**:
```rust
pub enum SyncCommand {
    NotifyLocalChange {
        id: String,
        data_type: SyncDataType,
        hash: String,
        ts: i64,
    },
    // ...
}

// When local entity changes:
tx.send(SyncCommand::NotifyLocalChange {
    id: agent_id,
    data_type: SyncDataType::Agent,
    hash: new_hash,
    ts: timestamp,
});

// In sync loop:
match cmd {
    SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
        let msg = json!({
            "type": "SYNC_ENTITY_UPDATE",
            "id": id,
            "dataType": data_type,
            "hash": hash,
            "ts": ts
        });
        ws_stream.send(Message::Text(msg.to_string())).await;
    }
}
```

### Soft Deletion for Offline Recovery

**Cleanup Old Deleted Records**:
```rust
async fn cleanup_old_deleted_records(app: &AppHandle, days: i64) {
    let threshold = now() - days * 24 * 60 * 60 * 1000;
    
    query("DELETE FROM agents WHERE deleted_at IS NOT NULL AND deleted_at < ?");
    query("DELETE FROM groups WHERE deleted_at IS NOT NULL AND deleted_at < ?");
    query("DELETE FROM topics WHERE deleted_at IS NOT NULL AND deleted_at < ?");
    query("DELETE FROM messages WHERE deleted_at IS NOT NULL AND deleted_at < ?");
}
```

**Retention Policy**: Keep soft-deleted records for configurable days (e.g., 30 days) before hard deletion.

### Pending Task Tracking

```rust
pub struct SyncState {
    pub pending_tasks: Arc<AtomicU32>,          // Phase 1 tasks
    pub pending_message_topics: Arc<AtomicU32>, // Phase 3 topics
}

// Increment on task start
pending_tasks.fetch_add(total_ops, Ordering::SeqCst);

// Decrement on task completion
if pending_tasks.fetch_sub(1, Ordering::SeqCst) == 1 {
    // All tasks complete - advance phase
    tx.send(SyncCommand::Phase1);
}
```

---

## Concrete Sync Scenarios

### Scenario 1: New Agent Created on Mobile

**Initial State**:
- Mobile: New agent "agent_001" created at ts=1712345678901
- Desktop: No knowledge of agent_001

**Sync Flow**:
```
1. Mobile: NotifyLocalChange { id: "agent_001", hash: "abc123", ts: 1712345678901 }

2. Phase 1:
   Mobile → Desktop: SYNC_MANIFEST { agent_001: { hash: "abc123", ts: 1712345678901 } }
   
   Desktop computes diff:
   - agent_001 not in local → action: PULL
   
   Desktop → Mobile: SYNC_DIFF_RESULTS { agent_001: "PULL" }

3. Mobile executes PUSH (since local has the data):
   Mobile → Desktop: POST /upload-entity { id: "agent_001", type: "agent", data: {...} }
   
4. Desktop:
   - Stores agent config
   - Computes config_hash
   - Initializes content_hash (empty, no topics yet)
   - Sends acknowledgment

5. Phase 1 completes → Phase 2 → Phase 3 → Sync complete
```

### Scenario 2: Conflict - Both Sides Modify Same Agent

**Initial State**:
- Mobile: agent_001 modified at ts=1712345690000, hash="def456"
- Desktop: agent_001 modified at ts=1712345695000, hash="ghi789"

**Sync Flow**:
```
1. Phase 1:
   Mobile → Desktop: SYNC_MANIFEST { agent_001: { hash: "def456", ts: 1712345690000 } }
   Desktop has: { hash: "ghi789", ts: 1712345695000 }
   
   Desktop computes diff:
   - local.hash ("ghi789") != remote.hash ("def456")
   - local.ts (1712345695000) > remote.ts (1712345690000)
   - action: PUSH (desktop is newer)
   
   Desktop → Mobile: SYNC_DIFF_RESULTS { agent_001: "PUSH" }

2. Mobile executes PULL:
   Mobile → Desktop: GET /download-entity?id=agent_001&type=agent
   Desktop → Mobile: AgentSyncDTO { ... }
   
3. Mobile:
   - Updates local agent config
   - Recomputes config_hash = "ghi789"
   - Updates timestamp to 1712345695000
   - Bubbles hash to parent

4. Result: Both sides now have hash="ghi789", ts=1712345695000
```

### Scenario 3: Timestamp Collision with Hash Arbitration

**Initial State**:
- Mobile: agent_001 modified at ts=1712345700000, hash="aaa111"
- Desktop: agent_001 modified at ts=1712345700000, hash="bbb222"

**Sync Flow**:
```
1. Phase 1:
   Mobile → Desktop: SYNC_MANIFEST { agent_001: { hash: "aaa111", ts: 1712345700000 } }
   Desktop has: { hash: "bbb222", ts: 1712345700000 }
   
   Desktop computes diff:
   - local.hash ("bbb222") != remote.hash ("aaa111")
   - local.ts (1712345700000) == remote.ts (1712345700000)
   - Timestamp collision!
   - Arbitration: "aaa111" < "bbb222" → true
   - action: PULL (remote/mobile wins)
   
   Desktop → Mobile: SYNC_DIFF_RESULTS { agent_001: "PULL" }

2. Desktop executes PULL:
   Desktop → Mobile: GET /download-entity?id=agent_001&type=agent
   Mobile → Desktop: AgentSyncDTO { ... }
   
3. Desktop updates to hash="aaa111"

4. Result: Both sides converge to hash="aaa111"
```

### Scenario 4: Deletion Propagation

**Initial State**:
- Mobile: agent_001 deleted at ts=1712345710000
- Desktop: agent_001 active

**Sync Flow**:
```
1. Phase 1:
   Mobile → Desktop: SYNC_MANIFEST { agent_001: { hash: "...", ts: ..., deletedAt: 1712345710000 } }
   Desktop has: agent_001 active (no deletedAt)
   
   Desktop computes diff:
   - Remote has deletedAt, local doesn't
   - action: DELETE { deletedAt: 1712345710000 }
   
   Desktop → Mobile: SYNC_DIFF_RESULTS { agent_001: "DELETE", deletedAt: 1712345710000 }

2. Mobile executes PUSH_DELETE:
   Mobile → Desktop: SYNC_DELETE_NOTIFY { id: "agent_001", dataType: "agent" }
   
3. Desktop receives notification:
   - Sets deleted_at = 1712345710000
   - Bubbles hash to mark parent as changed

4. Result: agent_001 soft-deleted on both sides
```

### Scenario 5: Message Sync with Attachments

**Initial State**:
- Mobile: topic_001 has 10 messages, 2 with attachments (hash="att001", "att002")
- Desktop: topic_001 has 8 messages, missing msg_009 and msg_010

**Sync Flow**:
```
1. Phase 3:
   Mobile → Desktop: GET_MESSAGE_MANIFEST { topicId: "topic_001" }
   
   Desktop → Mobile: MESSAGE_MANIFEST_RESULTS {
     topicId: "topic_001",
     messages: [
       { msgId: "msg_001", contentHash: "...", updatedAt: ... },
       ...
       { msgId: "msg_008", contentHash: "...", updatedAt: ... }
     ]
   }

2. Mobile computes diff:
   - msg_001-008: hashes match → skip
   - msg_009: not in remote → to_push = true
   - msg_010: not in remote → to_push = true
   
3. Mobile executes PUSH:
   Mobile → Desktop: POST /upload-messages {
     topicId: "topic_001",
     messages: [msg_009, msg_010 with attachment metadata]
   }
   
   Desktop → Mobile: { neededAttachmentHashes: ["att001", "att002"] }

4. Mobile uploads attachments:
   Mobile → Desktop: POST /upload-attachment?hash=att001&type=image/png
   Mobile → Desktop: POST /upload-attachment?hash=att002&type=application/pdf

5. Mobile updates hashes:
   - bubble_topic_hash("topic_001")
   - bubble_agent_hash("agent_001") or bubble_group_hash("group_001")

6. Result: Desktop now has all 10 messages with attachments
```

---

## Implementation Details

### Database Schema

```sql
-- Agents table
CREATE TABLE agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT,
    system_prompt TEXT,
    model TEXT,
    temperature REAL,
    context_token_limit INTEGER,
    max_output_tokens INTEGER,
    stream_output INTEGER,
    config_hash TEXT,        -- Hash of configuration
    content_hash TEXT,       -- Merkle root of topics
    created_at INTEGER,
    updated_at INTEGER,
    deleted_at INTEGER       -- Soft deletion timestamp
);

-- Groups table
CREATE TABLE groups (
    group_id TEXT PRIMARY KEY,
    name TEXT,
    mode TEXT,
    group_prompt TEXT,
    invite_prompt TEXT,
    use_unified_model INTEGER,
    unified_model TEXT,
    tag_match_mode TEXT,
    config_hash TEXT,
    content_hash TEXT,
    created_at INTEGER,
    updated_at INTEGER,
    deleted_at INTEGER
);

-- Topics table
CREATE TABLE topics (
    topic_id TEXT PRIMARY KEY,
    title TEXT,
    owner_id TEXT,           -- Agent or Group ID
    owner_type TEXT,         -- 'agent' or 'group'
    locked INTEGER,          -- UI state for agent topics
    unread INTEGER,          -- UI state for agent topics
    content_hash TEXT,       -- Merkle root of messages
    msg_count INTEGER,
    created_at INTEGER,
    updated_at INTEGER,
    deleted_at INTEGER
);

-- Messages table
CREATE TABLE messages (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT,
    role TEXT,
    content TEXT,
    timestamp INTEGER,
    content_hash TEXT,       -- Hash of content + attachments
    agent_id TEXT,
    group_id TEXT,
    updated_at INTEGER,
    deleted_at INTEGER
);

-- Attachments table
CREATE TABLE attachments (
    hash TEXT PRIMARY KEY,
    mime_type TEXT,
    size INTEGER,
    internal_path TEXT,
    created_at INTEGER
);

-- Message-Attachment junction table
CREATE TABLE message_attachments (
    msg_id TEXT,
    hash TEXT,
    PRIMARY KEY (msg_id, hash)
);

-- Avatars table
CREATE TABLE avatars (
    owner_type TEXT,         -- 'agent' or 'group'
    owner_id TEXT,
    image_data BLOB,
    mime_type TEXT,
    avatar_hash TEXT,
    dominant_color TEXT,
    updated_at INTEGER,
    deleted_at INTEGER,
    PRIMARY KEY (owner_type, owner_id)
);
```

### Hash Storage Strategy

**Two-Level Hashing**:
1. **config_hash**: Hash of configuration/metadata
2. **content_hash**: Merkle root of child entities

**Example - Agent**:
```
config_hash = SHA256(AgentSyncDTO)
content_hash = MerkleRoot([topic_001.content_hash, topic_002.content_hash, ...])
aggregate_hash = SHA256(config_hash || content_hash)
```

**Benefits**:
- Quick detection of config-only changes (no need to check topics)
- Efficient partial updates (only changed children affect content_hash)
- Hierarchical change propagation

### Idempotency Guarantees

**Idempotency Key Components**:
1. Action type (push/pull)
2. Entity type (agent/group/topic)
3. Entity ID
4. Time bucket (current minute)

**Example**:
```
Key for pushing agent_001 at 2024-04-05 10:30:45:
SHA256("push" + "agent" + "agent_001" + "1712345700")  // minute timestamp

Result: "a3f2b8c9d7e6f5a4..."
```

**Retry Behavior**:
- Same operation within the same minute → Same idempotency key
- Server recognizes duplicate and returns cached response
- Prevents duplicate entity creation on retries

---

## Performance Characteristics

### Network Efficiency

**Manifest Exchange**:
- Size: O(n) where n = number of entities
- Only transmits IDs, hashes, and timestamps
- Example: 100 agents → ~10KB manifest

**Delta Sync**:
- Only changed entities transferred
- Hash comparison identifies unchanged entities
- Typical sync: <5% of total data transferred

### Computational Complexity

**Hash Computation**:
- Single entity: O(1)
- Merkle aggregation: O(n log n) for sorting + O(n) for hashing
- Full manifest build: O(n) database queries + O(n log n) hash computation

**Diff Computation**:
- Build hash maps: O(n)
- Compare entities: O(n)
- Total: O(n)

### Concurrency Model

**Semaphore Limits**:
- Default: 10 concurrent operations
- Adaptive based on network type and success/failure streaks

**Parallel Execution**:
- Phase 1: All pull/push operations in parallel
- Phase 3: All topics in parallel
- Within topic: Messages pulled/pushed in batches

### Memory Usage

**In-Memory Structures**:
- Pending task counters: O(1)
- Uploaded hash tracker: O(m) where m = unique attachments
- Operation tracker: O(k) where k = concurrent operations

**Database Writes**:
- Batched via DbWriteQueue
- Reduces lock contention
- Sequential processing per queue

---

## Security Considerations

### Authentication

**Token-Based Auth**:
- WebSocket: Query parameter `?token=<sync_token>`
- HTTP: Header `x-sync-token: <sync_token>`

**Token Validation**:
- Server validates token on every request
- Invalid token → Connection rejected / 401 Unauthorized

### Data Integrity

**Hash Verification**:
- All entities have SHA-256 hashes
- Client can verify received data matches hash
- Detects data corruption in transit

**Idempotency Keys**:
- Prevents replay attacks within time window
- SHA-256 ensures keys cannot be forged

### Soft Deletion

**Benefits**:
- Recoverable deletions
- Audit trail (deleted_at timestamp)
- No accidental data loss from sync conflicts

**Cleanup Policy**:
- Configurable retention period
- Automatic hard deletion after threshold
- Prevents unbounded growth of deleted records

---

## Monitoring and Observability

### Sync Status Events

**Frontend Events**:
```javascript
// Connection status
window.addEventListener('vcp-sync-status', (event) => {
    console.log('Sync status:', event.detail.status);
    // Values: "connecting", "connected", "syncing", "completed", "error"
});

// Sync metrics
window.addEventListener('vcp-sync-metrics', (event) => {
    const { total, completed, failed, retries } = event.detail;
    console.log(`Progress: ${completed}/${total}, Failed: ${failed}, Retries: ${retries}`);
});

// Sync logs
window.addEventListener('vcp-log', (event) => {
    const { level, category, phase, message, sessionId, timestamp } = event.detail;
    if (level === 'error') {
        console.error(`[${phase}] ${message}`);
    }
});
```

### Console Logging

**Session Format**:
```
[SyncService] === Session sync_1712345678901_a3f2b8c9 started (log_level=Info) ===
[SyncService] [sync_1712345678901_a3f2b8c9] [INFO] [metadata] === Phase 1: Metadata ===
[SyncService] [sync_1712345678901_a3f2b8c9] [INFO] [metadata] agent:agent_001 - success (pulled from server)
[SyncService] [sync_1712345678901_a3f2b8c9] [ERROR] [metadata] agent:agent_002 - error (database locked)
[SyncService] [sync_1712345678901_a3f2b8c9] [INFO] [metadata] === Phase Complete: expected=10, success=8, errors=2, duration=1523ms ===
[SyncService] === Session sync_1712345678901_a3f2b8c9 ended ===
```

### Phase Metrics

**Tracked Metrics**:
- Expected operations count
- Success count
- Error count
- Phase duration
- Error details (retryable vs non-retryable)

**Example Summary**:
```json
{
  "phase": "metadata",
  "expected": 25,
  "success": 23,
  "errors": 2,
  "duration_ms": 3421,
  "error_summary": {
    "total": 2,
    "retryable": 1,
    "non_retryable": 1,
    "details": [
      {
        "id": "agent_002",
        "error": "database is locked",
        "timestamp": 1712345678901,
        "retryable": true
      },
      {
        "id": "agent_005",
        "error": "not found",
        "timestamp": 1712345678905,
        "retryable": false
      }
    ]
  }
}
```

---

## Conclusion

VCPMobile's synchronization architecture provides:

1. **Strong Consistency**: LWW + hash arbitration guarantees convergence
2. **Efficiency**: Delta sync via hash-based change detection
3. **Reliability**: Comprehensive retry mechanisms and error handling
4. **Scalability**: Parallel execution with adaptive concurrency
5. **Offline Support**: Automatic reconnection and change queueing
6. **Observability**: Detailed logging and metrics

The three-phase protocol ensures systematic synchronization of metadata, topics, and messages while maintaining referential integrity and minimizing data transfer.
