use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::is_valid_avatar_owner;

use tauri::{AppHandle, Manager, Runtime};

pub(crate) const MAX_AVATAR_BYTES: usize = 20 * 1024 * 1024;
const MAX_AVATAR_BATCH_BYTES: usize = 64 * 1024 * 1024;

const SAVE_AVATAR_SQL: &str =
    "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, dominant_color, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(owner_type, owner_id) DO UPDATE SET
        avatar_hash = excluded.avatar_hash,
        mime_type = excluded.mime_type,
        image_data = excluded.image_data,
        dominant_color = excluded.dominant_color,
        updated_at = excluded.updated_at
     WHERE avatars.deleted_at IS NULL";
const GET_AVATAR_SQL: &str = "SELECT mime_type, image_data, dominant_color, updated_at FROM avatars
     WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
       AND ((owner_type = 'user' AND owner_id = 'user_avatar')
         OR (owner_type = 'agent' AND EXISTS (
              SELECT 1 FROM agents WHERE agent_id = avatars.owner_id AND deleted_at IS NULL))
         OR (owner_type = 'group' AND EXISTS (
              SELECT 1 FROM groups WHERE group_id = avatars.owner_id AND deleted_at IS NULL)))";
const BATCH_AVATARS_SQL: &str =
    "SELECT owner_type, owner_id, mime_type, image_data, dominant_color, updated_at
     FROM avatars
     WHERE deleted_at IS NULL
       AND ((owner_type = 'user' AND owner_id = 'user_avatar')
         OR (owner_type = 'agent' AND EXISTS (
              SELECT 1 FROM agents WHERE agent_id = avatars.owner_id AND deleted_at IS NULL))
         OR (owner_type = 'group' AND EXISTS (
              SELECT 1 FROM groups WHERE group_id = avatars.owner_id AND deleted_at IS NULL)))";
const STORE_AVATAR_COLOR_SQL: &str = "UPDATE avatars SET dominant_color = ?
     WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL";

/// Tauri IPC Command: 保存头像二进制数据到数据库
/// 前端裁剪后将 Blob/ArrayBuffer 传给 Rust
#[tauri::command]
pub async fn save_avatar_data<R: Runtime>(
    app_handle: AppHandle<R>,
    owner_type: String,
    owner_id: String,
    mime_type: String,
    image_data: Vec<u8>,
) -> Result<String, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }
    if image_data.len() > MAX_AVATAR_BYTES {
        return Err(format!(
            "Avatar image exceeds the {MAX_AVATAR_BYTES}-byte limit"
        ));
    }

    // 1. 计算 SHA-256 哈希作为唯一标识
    let avatar_hash = crate::vcp_modules::infra::utils::calculate_sha256(&image_data);

    // 2. 预计算主色调 (Dominant Color)
    // 统一转交前端懒加载计算，后端落库阶段初始化为 None 以提升同步/存储性能与避开权限隐患
    let dominant_color: Option<String> = None;

    let now = crate::vcp_modules::infra::utils::now_millis();

    // 3. 在同一事务内验证 live parent 并写入，防止 orphan avatar。
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    let parent_is_live = match owner_type.as_str() {
        "agent" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE agent_id = ? AND deleted_at IS NULL)",
        )
        .bind(&owner_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?,
        "group" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE group_id = ? AND deleted_at IS NULL)",
        )
        .bind(&owner_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?,
        "user" => true,
        _ => false,
    };
    if !parent_is_live {
        return Err(format!(
            "Avatar owner {owner_type}/{owner_id} is missing or deleted"
        ));
    }

    let saved = sqlx::query(SAVE_AVATAR_SQL)
        .bind(&owner_type)
        .bind(&owner_id)
        .bind(&avatar_hash)
        .bind(&mime_type)
        .bind(&image_data)
        .bind(&dominant_color)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|e| e.to_string())?;
    if saved.rows_affected() != 1 {
        return Err(format!(
            "Avatar {owner_type}/{owner_id} is tombstoned and cannot be overwritten"
        ));
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;

    log::info!(
        "[AvatarService] Saved avatar for {} {}: hash={}, color={:?}",
        owner_type,
        owner_id,
        avatar_hash,
        dominant_color
    );

    Ok(avatar_hash)
}

#[derive(serde::Serialize)]
pub struct AvatarResult {
    pub mime_type: String,
    pub image_data: Vec<u8>,
    pub dominant_color: Option<String>,
    pub updated_at: i64,
}

/// Tauri IPC Command: 获取头像二进制数据
#[tauri::command]
pub async fn get_avatar<R: Runtime>(
    app_handle: AppHandle<R>,
    owner_type: String,
    owner_id: String,
) -> Result<Option<AvatarResult>, String> {
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let image_size: Option<i64> = sqlx::query_scalar(
        "SELECT LENGTH(image_data) FROM avatars
         WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
           AND ((owner_type = 'user' AND owner_id = 'user_avatar')
             OR (owner_type = 'agent' AND EXISTS (
                  SELECT 1 FROM agents WHERE agent_id = avatars.owner_id AND deleted_at IS NULL))
             OR (owner_type = 'group' AND EXISTS (
                  SELECT 1 FROM groups WHERE group_id = avatars.owner_id AND deleted_at IS NULL)))",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    let Some(image_size) = image_size else {
        return Ok(None);
    };
    let image_size = usize::try_from(image_size)
        .map_err(|_| format!("Avatar {owner_type}/{owner_id} has an invalid image size"))?;
    if image_size > MAX_AVATAR_BYTES {
        return Err(format!(
            "Avatar {owner_type}/{owner_id} exceeds the {MAX_AVATAR_BYTES}-byte limit"
        ));
    }

    let row_res = sqlx::query(GET_AVATAR_SQL)
        .bind(&owner_type)
        .bind(&owner_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = row_res {
        use sqlx::Row;
        Ok(Some(AvatarResult {
            mime_type: row
                .try_get("mime_type")
                .map_err(|error| format!("Avatar MIME decode failed: {error}"))?,
            image_data: row
                .try_get("image_data")
                .map_err(|error| format!("Avatar image decode failed: {error}"))?,
            dominant_color: row
                .try_get("dominant_color")
                .map_err(|error| format!("Avatar color decode failed: {error}"))?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|error| format!("Avatar timestamp decode failed: {error}"))?,
        }))
    } else {
        Ok(None)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAvatarItem {
    pub owner_type: String,
    pub owner_id: String,
    pub mime_type: String,
    pub image_data: Vec<u8>,
    pub dominant_color: Option<String>,
    pub updated_at: i64,
}

/// Tauri IPC Command: 批量获取所有头像二进制数据
#[tauri::command]
pub async fn batch_get_avatars<R: Runtime>(
    app_handle: AppHandle<R>,
) -> Result<Vec<BatchAvatarItem>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let size_row = sqlx::query(
        "SELECT COALESCE(MAX(LENGTH(image_data)), 0) AS max_bytes,
                COALESCE(SUM(LENGTH(image_data)), 0) AS total_bytes
         FROM avatars
         WHERE deleted_at IS NULL
           AND ((owner_type = 'user' AND owner_id = 'user_avatar')
             OR (owner_type = 'agent' AND EXISTS (
                  SELECT 1 FROM agents WHERE agent_id = avatars.owner_id AND deleted_at IS NULL))
             OR (owner_type = 'group' AND EXISTS (
                  SELECT 1 FROM groups WHERE group_id = avatars.owner_id AND deleted_at IS NULL)))",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| error.to_string())?;
    use sqlx::Row;
    let max_bytes: i64 = size_row
        .try_get("max_bytes")
        .map_err(|error| format!("Avatar max size decode failed: {error}"))?;
    let total_bytes: i64 = size_row
        .try_get("total_bytes")
        .map_err(|error| format!("Avatar total size decode failed: {error}"))?;
    let max_bytes = usize::try_from(max_bytes)
        .map_err(|_| "Avatar batch contains an invalid image size".to_string())?;
    let total_bytes = usize::try_from(total_bytes)
        .map_err(|_| "Avatar batch contains an invalid total size".to_string())?;
    if max_bytes > MAX_AVATAR_BYTES || total_bytes > MAX_AVATAR_BATCH_BYTES {
        return Err(format!(
            "Avatar batch exceeds the per-item ({MAX_AVATAR_BYTES}) or total ({MAX_AVATAR_BATCH_BYTES}) byte limit"
        ));
    }

    let rows = sqlx::query(BATCH_AVATARS_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        results.push(BatchAvatarItem {
            owner_type: row
                .try_get("owner_type")
                .map_err(|error| format!("Avatar owner type decode failed: {error}"))?,
            owner_id: row
                .try_get("owner_id")
                .map_err(|error| format!("Avatar owner id decode failed: {error}"))?,
            mime_type: row
                .try_get("mime_type")
                .map_err(|error| format!("Avatar MIME decode failed: {error}"))?,
            image_data: row
                .try_get("image_data")
                .map_err(|error| format!("Avatar image decode failed: {error}"))?,
            dominant_color: row
                .try_get("dominant_color")
                .map_err(|error| format!("Avatar color decode failed: {error}"))?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|error| format!("Avatar timestamp decode failed: {error}"))?,
        });
    }

    Ok(results)
}

/// Tauri IPC Command: 为已有头像存储前端计算好的 dominant_color
#[tauri::command]
pub async fn store_dominant_color(
    db_state: tauri::State<'_, DbState>,
    owner_type: String,
    owner_id: String,
    color: String,
) -> Result<(), String> {
    let pool = &db_state.pool;
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }

    let updated = sqlx::query(STORE_AVATAR_COLOR_SQL)
        .bind(&color)
        .bind(&owner_type)
        .bind(&owner_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    if updated.rows_affected() != 1 {
        return Err(format!(
            "Live avatar {owner_type}/{owner_id} does not exist"
        ));
    }

    log::info!(
        "[AvatarService] Stored frontend-computed dominant_color for {} {}: {}",
        owner_type,
        owner_id,
        color
    );

    Ok(())
}

/// 从字节数组中提取主色调 (公开供协议层兜底使用)
/// 策略：后端已将主色调计算彻底移交给前端，此处仅保留极简 O(1) 的纯灰色 `#808080` 兜底实现，杜绝 ffmpeg 进程与权限报错
#[allow(dead_code)]
pub fn extract_dominant_color_from_bytes(_data: &[u8]) -> Result<String, String> {
    Ok("#808080".to_string())
}

#[cfg(test)]
mod tests {
    use super::{BATCH_AVATARS_SQL, GET_AVATAR_SQL, SAVE_AVATAR_SQL, STORE_AVATAR_COLOR_SQL};
    use sqlx::Row;

    #[tokio::test]
    async fn avatar_tombstones_are_hidden_and_cannot_be_silently_overwritten() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE avatars (
                owner_type TEXT, owner_id TEXT, avatar_hash TEXT, mime_type TEXT,
                image_data BLOB, dominant_color TEXT, updated_at BIGINT, deleted_at BIGINT,
                PRIMARY KEY(owner_type, owner_id)
             );
             CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at BIGINT);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at BIGINT);
             INSERT INTO agents VALUES ('deleted', 2), ('live', NULL);
             INSERT INTO avatars VALUES
                ('agent', 'deleted', 'old', 'image/png', X'01', '#111111', 1, 2),
                ('agent', 'live', 'live', 'image/png', X'02', '#222222', 1, NULL),
                ('user', 'user_avatar', 'user', 'image/png', X'03', NULL, 1, NULL),
                ('user', 'invalid-user', 'bad', 'image/png', X'04', NULL, 1, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        assert!(sqlx::query(GET_AVATAR_SQL)
            .bind("agent")
            .bind("deleted")
            .fetch_optional(&pool)
            .await
            .expect("read tombstone")
            .is_none());
        let batch = sqlx::query(BATCH_AVATARS_SQL)
            .fetch_all(&pool)
            .await
            .expect("read batch");
        assert_eq!(batch.len(), 2);
        let owner_ids = batch
            .iter()
            .map(|row| row.get::<String, _>("owner_id"))
            .collect::<std::collections::HashSet<_>>();
        assert!(owner_ids.contains("live"));
        assert!(owner_ids.contains("user_avatar"));
        assert!(!owner_ids.contains("invalid-user"));

        let save = sqlx::query(SAVE_AVATAR_SQL)
            .bind("agent")
            .bind("deleted")
            .bind("new")
            .bind("image/png")
            .bind(vec![3u8])
            .bind(Option::<String>::None)
            .bind(3i64)
            .execute(&pool)
            .await
            .expect("guarded save");
        assert_eq!(save.rows_affected(), 0);
        let color = sqlx::query(STORE_AVATAR_COLOR_SQL)
            .bind("#ffffff")
            .bind("agent")
            .bind("deleted")
            .execute(&pool)
            .await
            .expect("guarded color update");
        assert_eq!(color.rows_affected(), 0);
    }
}
