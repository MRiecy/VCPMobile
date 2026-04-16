use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_types::SyncManifest;
use sqlx::SqlitePool;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_agent_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        ManifestBuilder::build_agent_manifest(pool).await
    }

    pub async fn build_group_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        ManifestBuilder::build_group_manifest(pool).await
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        ManifestBuilder::build_avatar_manifest(pool).await
    }

    pub async fn build_all_manifests(pool: &SqlitePool) -> Result<Vec<SyncManifest>, String> {
        ManifestBuilder::build_all_manifests(pool).await
    }
}
