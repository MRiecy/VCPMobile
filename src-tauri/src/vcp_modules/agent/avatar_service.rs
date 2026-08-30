use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::is_valid_avatar_owner;

use tauri::{AppHandle, Manager, Runtime};

pub(crate) const MAX_AVATAR_BYTES: usize = 20 * 1024 * 1024;

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
const GET_AVATAR_SQL: &str = "SELECT avatar_hash, mime_type, image_data, dominant_color, updated_at FROM avatars
     WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
       AND ((owner_type = 'user' AND owner_id = 'user_avatar')
         OR (owner_type = 'agent' AND EXISTS (
              SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = avatars.owner_id AND deleted_at IS NULL))
         OR (owner_type = 'group' AND EXISTS (
              SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = avatars.owner_id AND deleted_at IS NULL)))";
const BATCH_AVATARS_SQL: &str =
    "SELECT owner_type, owner_id, avatar_hash, dominant_color, updated_at
     FROM avatars
     WHERE deleted_at IS NULL
       AND ((owner_type = 'user' AND owner_id = 'user_avatar')
         OR (owner_type = 'agent' AND EXISTS (
              SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = avatars.owner_id AND deleted_at IS NULL))
         OR (owner_type = 'group' AND EXISTS (
              SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = avatars.owner_id AND deleted_at IS NULL)))";
const STORE_AVATAR_COLOR_SQL: &str = "UPDATE avatars SET dominant_color = ?
     WHERE owner_type = ? AND owner_id = ? AND avatar_hash = ?
       AND dominant_color IS NULL AND deleted_at IS NULL";

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
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }
    if image_data.is_empty() {
        return Err("Avatar image must not be empty".to_string());
    }
    if !mime_type.starts_with("image/") {
        return Err(format!("Invalid avatar MIME type {mime_type}"));
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
    let mut transaction = db_state.write_transaction("identity.avatar.save").await?;
    let parent_is_live = match owner_type.as_str() {
        "agent" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL)",
        )
        .bind(&owner_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?,
        "group" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL)",
        )
        .bind(&owner_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?,
        "user" => true,
        _ => false,
    };
    if !parent_is_live {
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
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
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
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
    pub avatar_hash: String,
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
                  SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = avatars.owner_id AND deleted_at IS NULL))
             OR (owner_type = 'group' AND EXISTS (
                  SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = avatars.owner_id AND deleted_at IS NULL)))",
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
            avatar_hash: row
                .try_get("avatar_hash")
                .map_err(|error| format!("Avatar hash decode failed: {error}"))?,
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
pub struct BatchAvatarMetadata {
    pub owner_type: String,
    pub owner_id: String,
    pub avatar_hash: String,
    pub dominant_color: Option<String>,
    pub updated_at: i64,
}

/// Tauri IPC Command: 批量获取头像元数据；二进制由可视区组件按需读取。
#[tauri::command]
pub async fn batch_get_avatars<R: Runtime>(
    app_handle: AppHandle<R>,
) -> Result<Vec<BatchAvatarMetadata>, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let rows = sqlx::query(BATCH_AVATARS_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    use sqlx::Row;
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        results.push(BatchAvatarMetadata {
            owner_type: row
                .try_get("owner_type")
                .map_err(|error| format!("Avatar owner type decode failed: {error}"))?,
            owner_id: row
                .try_get("owner_id")
                .map_err(|error| format!("Avatar owner id decode failed: {error}"))?,
            avatar_hash: row
                .try_get("avatar_hash")
                .map_err(|error| format!("Avatar hash decode failed: {error}"))?,
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
    expected_avatar_hash: String,
) -> Result<bool, String> {
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }

    let mut tx = db_state.write_transaction("identity.avatar.color").await?;
    let updated = sqlx::query(STORE_AVATAR_COLOR_SQL)
        .bind(&color)
        .bind(&owner_type)
        .bind(&owner_id)
        .bind(&expected_avatar_hash)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    if updated.rows_affected() == 0 {
        log::debug!(
            "[AvatarService] Ignored stale dominant_color for {} {} at hash {}",
            owner_type,
            owner_id,
            expected_avatar_hash
        );
        return Ok(false);
    }

    log::info!(
        "[AvatarService] Stored frontend-computed dominant_color for {} {}: {}",
        owner_type,
        owner_id,
        color
    );

    Ok(true)
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
             CREATE TABLE agents (
                owner_type TEXT, agent_id TEXT, deleted_at BIGINT,
                PRIMARY KEY(owner_type, agent_id)
             );
             CREATE TABLE groups (
                owner_type TEXT, group_id TEXT, deleted_at BIGINT,
                PRIMARY KEY(owner_type, group_id)
             );
             INSERT INTO agents VALUES
                ('agent', 'deleted', 2), ('agent', 'live', NULL);
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
            .bind("old")
            .execute(&pool)
            .await
            .expect("guarded color update");
        assert_eq!(color.rows_affected(), 0);

        let stale_color = sqlx::query(STORE_AVATAR_COLOR_SQL)
            .bind("#abcdef")
            .bind("user")
            .bind("user_avatar")
            .bind("stale")
            .execute(&pool)
            .await
            .expect("stale color update");
        assert_eq!(stale_color.rows_affected(), 0);

        let current_color = sqlx::query(STORE_AVATAR_COLOR_SQL)
            .bind("#abcdef")
            .bind("user")
            .bind("user_avatar")
            .bind("user")
            .execute(&pool)
            .await
            .expect("current color update");
        assert_eq!(current_color.rows_affected(), 1);
    }
}
