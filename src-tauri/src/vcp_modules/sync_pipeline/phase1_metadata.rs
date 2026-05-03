use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_types::SyncManifest;
use sqlx::SqlitePool;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_phase1_manifests(pool: &SqlitePool) -> Result<Vec<SyncManifest>, String> {
        ManifestBuilder::build_phase1_manifests(pool).await
    }

    pub async fn build_topic_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        ManifestBuilder::build_topic_manifest(pool).await
    }
}
