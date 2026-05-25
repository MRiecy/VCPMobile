use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseProgress {
    pub total: u32,
    pub completed: u32,
    pub pending: Vec<String>,
}

impl PhaseProgress {
    pub fn new() -> Self {
        Self {
            total: 0,
            completed: 0,
            pending: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum PipelinePhase {
    #[default]
    Idle,
    Phase1Metadata {
        progress: PhaseProgress,
    },
    Phase2TopicMetadata {
        progress: PhaseProgress,
    },
    Phase3Messages {
        progress: PhaseProgress,
    },
    Completed,
    Failed {
        error: String,
        phase: String,
    },
}

pub enum PipelineCommand {
    StartTopicMetadata,   // Phase 2: Pull missing configs
    StartTopicValidation, // Phase 2.5: Dual-hash check
    StartMessages,        // Phase 3: Message diff
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

    #[allow(dead_code)]
    pub fn get_state(&self) -> Arc<RwLock<PipelinePhase>> {
        self.state.clone()
    }

    /// 进入 Phase 2: Topic 元数据补全
    pub async fn on_owner_metadata_done(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Phase2TopicMetadata {
                progress: PhaseProgress::new(),
            };
        }
        let _ = self.command_tx.send(PipelineCommand::StartTopicMetadata);
        Ok(())
    }

    /// 进入 Phase 2.5: Topic 哈希比对
    pub async fn on_topic_metadata_pull_done(&self) -> Result<(), String> {
        // 哈希比对在 Phase2 逻辑内，不改变底层 PipelinePhase 枚举
        let _ = self.command_tx.send(PipelineCommand::StartTopicValidation);
        Ok(())
    }

    /// 进入 Phase 3: 消息同步
    pub async fn on_topic_validation_done(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Phase3Messages {
                progress: PhaseProgress::new(),
            };
        }
        let _ = self.command_tx.send(PipelineCommand::StartMessages);
        Ok(())
    }

    /// 同步结束
    pub async fn on_messages_done(&self) -> Result<(), String> {
        {
            let mut state = self.state.write().await;
            *state = PipelinePhase::Completed;
        }
        let _ = self.command_tx.send(PipelineCommand::Finalize);
        Ok(())
    }
}
