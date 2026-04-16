use crate::vcp_modules::sync_pipeline::pipeline_state::{PipelinePhase, PhaseProgress};
use crate::vcp_modules::sync_hash::HashInitializer;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub enum PipelineCommand {
    StartFullSync,
    Phase1Completed,
    Phase2Completed,
    Phase3Completed,
    PhaseFailed { error: String, phase: String },
}

pub struct SyncPipeline {
    state: Arc<RwLock<PipelinePhase>>,
    command_tx: mpsc::UnboundedSender<PipelineCommand>,
    metrics: Arc<SyncPipelineMetrics>,
}

#[derive(Default)]
pub struct SyncPipelineMetrics {
    pub phase1_duration_ms: Arc<RwLock<Option<u64>>>,
    pub phase2_duration_ms: Arc<RwLock<Option<u64>>>,
    pub phase3_duration_ms: Arc<RwLock<Option<u64>>>,
    pub total_sync_count: Arc<RwLock<u32>>,
    pub failed_sync_count: Arc<RwLock<u32>>,
}

impl SyncPipeline {
    pub fn new(command_tx: mpsc::UnboundedSender<PipelineCommand>) -> Self {
        Self {
            state: Arc::new(RwLock::new(PipelinePhase::Idle)),
            command_tx,
            metrics: Arc::new(SyncPipelineMetrics::default()),
        }
    }

    pub fn state(&self) -> Arc<RwLock<PipelinePhase>> {
        self.state.clone()
    }

    pub fn metrics(&self) -> Arc<SyncPipelineMetrics> {
        self.metrics.clone()
    }

    pub async fn start_full_sync(&self) -> Result<(), String> {
        {
            let state = self.state.read().await;
            if state.is_running() {
                return Err("Pipeline already running".to_string());
            }
        }

        self.transition_to_phase1().await?;
        let _ = self.command_tx.send(PipelineCommand::StartFullSync);
        Ok(())
    }

    pub async fn on_phase1_completed(&self, pool: &sqlx::SqlitePool) -> Result<(), String> {
        HashInitializer::ensure_all_agent_hashes(pool).await?;
        HashInitializer::ensure_all_group_hashes(pool).await?;

        self.transition_to_phase2().await?;
        let _ = self.command_tx.send(PipelineCommand::Phase1Completed);
        Ok(())
    }

    pub async fn on_phase2_completed(&self) -> Result<(), String> {
        self.transition_to_phase3().await?;
        let _ = self.command_tx.send(PipelineCommand::Phase2Completed);
        Ok(())
    }

    pub async fn on_phase3_completed(&self) -> Result<(), String> {
        self.transition_to_completed().await?;
        let _ = self.command_tx.send(PipelineCommand::Phase3Completed);
        Ok(())
    }

    pub async fn on_phase_failed(&self, error: String, phase: String) {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Failed { error: error.clone(), phase: phase.clone() };
        }
        let _ = self.command_tx.send(PipelineCommand::PhaseFailed { error, phase });
    }

    async fn transition_to_phase1(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase1Metadata {
            progress: PhaseProgress::new(),
        };
        println!("[SyncPipeline] Transitioned to Phase1: Metadata Sync");
        Ok(())
    }

    async fn transition_to_phase2(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase2Topic {
            progress: PhaseProgress::new(),
        };
        println!("[SyncPipeline] Transitioned to Phase2: Topic Sync");
        Ok(())
    }

    async fn transition_to_phase3(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase3Message {
            progress: PhaseProgress::new(),
        };
        println!("[SyncPipeline] Transitioned to Phase3: Message Sync");
        Ok(())
    }

    async fn transition_to_completed(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Completed;
        println!("[SyncPipeline] Pipeline Completed");
        Ok(())
    }

    pub async fn set_phase1_progress(&self, progress: PhaseProgress) {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase1Metadata { progress };
    }

    pub async fn set_phase2_progress(&self, progress: PhaseProgress) {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase2Topic { progress };
    }

    pub async fn set_phase3_progress(&self, progress: PhaseProgress) {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase3Message { progress };
    }

    pub async fn reset_to_idle(&self) {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Idle;
    }
}
