use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, AvatarManifestDeleted, AvatarManifestLive, AvatarManifestState,
    AvatarOwnerType, ManifestRequest, OwnerManifestDeleted, OwnerManifestLive, OwnerManifestState,
    OwnerType, TopicManifestDeleted, TopicManifestLive, TopicManifestState,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashSet;

const SQLITE_BIND_CHUNK: usize = 400;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_owner_manifest(pool: &SqlitePool) -> Result<ManifestRequest, String> {
        let rows = sqlx::query(
            "SELECT owner_type, agent_id AS owner_id, config_hash, content_hash,
                    updated_at, deleted_at
             FROM agents WHERE owner_type = 'agent'
             UNION ALL
             SELECT owner_type, group_id AS owner_id, config_hash, content_hash,
                    updated_at, deleted_at
             FROM groups WHERE owner_type = 'group'
             ORDER BY owner_type, owner_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let raw_owner_type: String = r
                .try_get("owner_type")
                .map_err(|error| format!("Owner manifest type decode failed: {error}"))?;
            let owner_type = OwnerType::try_from(raw_owner_type.as_str()).map_err(|_| {
                format!("Owner manifest has unsupported owner type {raw_owner_type}")
            })?;
            let owner_id: String = r
                .try_get("owner_id")
                .map_err(|error| format!("Owner manifest id decode failed: {error}"))?;
            if owner_id.is_empty() {
                return Err("Owner manifest has an empty owner id".to_string());
            }
            let deleted_at: Option<i64> = r
                .try_get("deleted_at")
                .map_err(|error| format!("Owner manifest tombstone decode failed: {error}"))?;
            if let Some(deleted_at) = deleted_at {
                items.push(OwnerManifestState::Deleted(OwnerManifestDeleted {
                    owner_type,
                    owner_id,
                    deleted_at,
                }));
                continue;
            }
            let config_hash: String = r.try_get("config_hash").map_err(|error| {
                format!(
                    "Owner manifest config hash decode failed for {owner_type}/{owner_id}: {error}"
                )
            })?;
            let content_hash: String = r.try_get("content_hash").map_err(|error| {
                format!(
                    "Owner manifest content hash decode failed for {owner_type}/{owner_id}: {error}"
                )
            })?;
            items.push(OwnerManifestState::Live(OwnerManifestLive {
                owner_type,
                owner_id,
                config_hash,
                content_hash,
                updated_at: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Owner manifest timestamp decode failed: {error}"))?,
            }));
        }

        Ok(ManifestRequest::Owner { items })
    }

    pub async fn build_targeted_topic_manifest(
        pool: &SqlitePool,
        owners: &[OwnerKey],
    ) -> Result<ManifestRequest, String> {
        if owners.is_empty() {
            return Ok(ManifestRequest::Topic {
                items: Vec::new(),
                targeted_owners: Vec::new(),
            });
        }

        let expected_owners = owners.iter().cloned().collect::<HashSet<_>>();
        if expected_owners.len() != owners.len()
            || expected_owners.iter().any(|owner| {
                !matches!(owner.owner_type.as_str(), "agent" | "group") || owner.owner_id.is_empty()
            })
        {
            return Err("Targeted topic manifest contains invalid or duplicate owners".into());
        }

        let mut items = Vec::new();
        let mut seen_topics = HashSet::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let predicates = owner_chunk
                .iter()
                .map(|_| "(owner_type = ? AND owner_id = ?)")
                .collect::<Vec<_>>()
                .join(" OR ");
            let query_str = format!(
                "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, owner_id, deleted_at
                 FROM topics WHERE {predicates}"
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(query_str));
            for owner in owner_chunk {
                query = query.bind(&owner.owner_type).bind(&owner.owner_id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Topic manifest id decode failed: {error}"))?;
                let raw_owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Topic manifest owner type decode failed for {topic_id}: {error}")
                })?;
                let owner_type = OwnerType::try_from(raw_owner_type.as_str()).map_err(|_| {
                    format!("Topic manifest {topic_id} has unsupported owner type {raw_owner_type}")
                })?;
                let owner_id: String = row.try_get("owner_id").map_err(|error| {
                    format!("Topic manifest owner id decode failed for {topic_id}: {error}")
                })?;
                let topic_key = TopicKey::new(owner_type.as_str(), &owner_id, &topic_id);
                if owner_id.is_empty()
                    || !expected_owners.contains(&OwnerKey::new(owner_type.as_str(), &owner_id))
                {
                    return Err(format!(
                        "Topic manifest {topic_id} returned unexpected owner {owner_type}/{owner_id}"
                    ));
                }
                if !seen_topics.insert(topic_key) {
                    return Err(format!(
                        "Targeted topic manifest returned duplicate topic {owner_type}/{owner_id}/{topic_id}"
                    ));
                }
                let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(|error| {
                    format!("Topic manifest tombstone decode failed for {topic_id}: {error}")
                })?;
                if let Some(deleted_at) = deleted_at {
                    items.push(TopicManifestState::Deleted(TopicManifestDeleted {
                        owner_type,
                        owner_id,
                        topic_id,
                        deleted_at,
                    }));
                    continue;
                }
                let config_hash: String = row.try_get("config_hash").map_err(|error| {
                    format!("Topic manifest config hash decode failed for {topic_id}: {error}")
                })?;
                let content_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Topic manifest content hash decode failed for {topic_id}: {error}")
                })?;
                let updated_at = row.try_get("updated_at").map_err(|error| {
                    format!("Topic manifest timestamp decode failed for {topic_id}: {error}")
                })?;
                items.push(TopicManifestState::Live(TopicManifestLive {
                    owner_type,
                    owner_id,
                    topic_id,
                    config_hash,
                    content_hash,
                    updated_at,
                }));
            }
        }

        Ok(ManifestRequest::Topic {
            items,
            targeted_owners: owners.to_vec(),
        })
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<ManifestRequest, String> {
        let rows = sqlx::query(
            "SELECT av.owner_id, av.owner_type, av.avatar_hash, av.updated_at, av.deleted_at,
                    CASE
                        WHEN av.deleted_at IS NOT NULL THEN 1
                        WHEN av.owner_type = 'user' THEN 1
                        WHEN av.owner_type = 'agent' AND agent.agent_id IS NOT NULL THEN 1
                        WHEN av.owner_type = 'group' AND owner_group.group_id IS NOT NULL THEN 1
                        ELSE 0
                    END AS parent_is_live
             FROM avatars av
             LEFT JOIN agents agent
               ON av.owner_type = 'agent'
              AND agent.owner_type = 'agent'
              AND agent.agent_id = av.owner_id
              AND agent.deleted_at IS NULL
             LEFT JOIN groups owner_group
               ON av.owner_type = 'group'
              AND owner_group.owner_type = 'group'
              AND owner_group.group_id = av.owner_id
              AND owner_group.deleted_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let raw_owner_type: String = r
                .try_get("owner_type")
                .map_err(|error| format!("Avatar manifest owner type decode failed: {error}"))?;
            let owner_id: String = r
                .try_get("owner_id")
                .map_err(|error| format!("Avatar manifest owner id decode failed: {error}"))?;
            if !is_valid_avatar_owner(&raw_owner_type, &owner_id) {
                return Err(format!(
                    "Avatar manifest has invalid owner {raw_owner_type}/{owner_id}"
                ));
            }
            let owner_type = AvatarOwnerType::try_from(raw_owner_type.as_str()).map_err(|_| {
                format!("Avatar manifest has invalid owner {raw_owner_type}/{owner_id}")
            })?;
            let deleted_at: Option<i64> = r
                .try_get("deleted_at")
                .map_err(|error| format!("Avatar manifest tombstone decode failed: {error}"))?;
            let parent_is_live: bool = r.try_get("parent_is_live").map_err(|error| {
                format!(
                    "Avatar manifest parent state decode failed for {owner_type}/{owner_id}: {error}"
                )
            })?;
            if !parent_is_live {
                return Err(format!(
                    "Avatar manifest owner {owner_type}/{owner_id} is missing or deleted"
                ));
            }
            if let Some(deleted_at) = deleted_at {
                items.push(AvatarManifestState::Deleted(AvatarManifestDeleted {
                    owner_type,
                    owner_id,
                    deleted_at,
                }));
                continue;
            }
            let binary_hash = r.try_get("avatar_hash").map_err(|error| {
                format!("Avatar manifest hash decode failed for {owner_type}/{owner_id}: {error}")
            })?;
            items.push(AvatarManifestState::Live(AvatarManifestLive {
                owner_type,
                owner_id,
                binary_hash,
                updated_at: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Avatar manifest timestamp decode failed: {error}"))?,
            }));
        }

        Ok(ManifestRequest::Avatar { items })
    }
}

#[cfg(test)]
mod tests {
    use super::Phase1Metadata;
    use crate::vcp_modules::sync_types::{
        AvatarManifestState, ManifestRequest, OwnerManifestState, TopicManifestState,
    };
    use crate::vcp_modules::topic_types::OwnerKey;

    #[tokio::test]
    async fn owner_manifest_combines_agent_and_group_states() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE agents (
                owner_type TEXT, agent_id TEXT, config_hash TEXT,
                content_hash TEXT, updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE groups (
                owner_type TEXT, group_id TEXT, config_hash TEXT,
                content_hash TEXT, updated_at INTEGER, deleted_at INTEGER
             );
             INSERT INTO agents VALUES
                ('agent', 'agent-a', 'agent-config', 'agent-content', 10, NULL);
             INSERT INTO groups VALUES
                ('group', 'group-a', 'group-config', 'group-content', 11, 9);",
        )
        .execute(&pool)
        .await
        .expect("create owner fixture");

        let manifest = Phase1Metadata::build_owner_manifest(&pool)
            .await
            .expect("build owner manifest");
        let ManifestRequest::Owner { items } = manifest else {
            panic!("owner builder returned another manifest type");
        };
        assert_eq!(items.len(), 2);
        let OwnerManifestState::Live(agent) = &items[0] else {
            panic!("agent should be live");
        };
        assert_eq!(agent.owner_id, "agent-a");
        let OwnerManifestState::Deleted(group) = &items[1] else {
            panic!("group should be deleted");
        };
        assert_eq!(group.owner_id, "group-a");
        assert_eq!(group.deleted_at, 9);
    }

    #[tokio::test]
    async fn avatar_manifest_preserves_tombstones() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE avatars (
                owner_id TEXT, owner_type TEXT, avatar_hash TEXT,
                updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE agents (
                owner_type TEXT, agent_id TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                owner_type TEXT, group_id TEXT, deleted_at INTEGER
             );
             INSERT INTO avatars VALUES
                ('agent-a', 'agent', 'hash', 10, 9),
                ('user_avatar', 'user', 'user-hash', 11, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create avatar fixture");

        let manifest = Phase1Metadata::build_avatar_manifest(&pool)
            .await
            .expect("build avatar manifest");
        let ManifestRequest::Avatar { items } = manifest else {
            panic!("avatar builder returned another manifest type");
        };
        assert_eq!(items.len(), 2);
        let AvatarManifestState::Deleted(agent) = &items[0] else {
            panic!("agent avatar should be deleted");
        };
        assert_eq!(agent.owner_id, "agent-a");
        assert_eq!(agent.deleted_at, 9);
        let AvatarManifestState::Live(user) = &items[1] else {
            panic!("user avatar should be live");
        };
        assert_eq!(user.owner_id, "user_avatar");
        assert_eq!(user.binary_hash, "user-hash");
    }

    #[tokio::test]
    async fn targeted_topic_manifest_carries_exact_owner_identity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                config_hash TEXT, content_hash TEXT, updated_at INTEGER,
                deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             INSERT INTO topics VALUES
                ('agent', 'agent-a', 'topic-a', 'config-hash',
                 'content-hash', 10, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create topic fixture");

        let manifest = Phase1Metadata::build_targeted_topic_manifest(
            &pool,
            &[OwnerKey::new("agent", "agent-a")],
        )
        .await
        .expect("build topic manifest");
        let ManifestRequest::Topic {
            items,
            targeted_owners,
        } = manifest
        else {
            panic!("topic builder returned another manifest type");
        };
        assert_eq!(targeted_owners, vec![OwnerKey::new("agent", "agent-a")]);
        assert_eq!(items.len(), 1);
        let TopicManifestState::Live(topic) = &items[0] else {
            panic!("topic should be live");
        };
        assert_eq!(topic.topic_id, "topic-a");
        assert_eq!(topic.owner_id, "agent-a");
        assert_eq!(topic.config_hash, "config-hash");
        assert_eq!(topic.content_hash, "content-hash");
    }
}
