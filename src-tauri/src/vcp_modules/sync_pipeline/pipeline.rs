use crate::vcp_modules::sync_pipeline::pipeline_state::{PhaseProgress, PipelinePhase};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub enum PipelineCommand {
    StartTopicHash,
    StartMessageDiff,
    Finalize,
}

pub struct SyncPipeline {
    state: Arc<RwLock<PipelinePhase>>,
    command_tx: mpsc::UnboundedSender<PipelineCommand>,
}

impl SyncPipeline {
    pub fn new(command_tx: mpsc::UnboundedSender<PipelineCommand>) -> Self {
        Self {
            state: Arc::new(RwLock::new(PipelinePhase::Idle)),
            command_tx,
        }
    }

    pub async fn on_phase1_completed(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Phase2Topic {
                progress: PhaseProgress::new(),
            };
        }
        let _ = self.command_tx.send(PipelineCommand::StartTopicHash);
        Ok(())
    }

    pub async fn on_phase2_completed(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Phase3Message {
                progress: PhaseProgress::new(),
            };
        }
        let _ = self.command_tx.send(PipelineCommand::StartMessageDiff);
        Ok(())
    }

    pub async fn on_phase3_completed(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Completed;
        }
        let _ = self.command_tx.send(PipelineCommand::Finalize);
        Ok(())
    }
}
