use crate::vcp_modules::sync_manifest::ManifestBuilder;
use crate::vcp_modules::sync_types::SyncManifest;
use sqlx::SqlitePool;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_all_manifests(pool: &SqlitePool) -> Result<Vec<SyncManifest>, String> {
        ManifestBuilder::build_all_manifests(pool).await
    }
}
