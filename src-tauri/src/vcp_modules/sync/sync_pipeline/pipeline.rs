use tokio::sync::mpsc;

#[allow(clippy::enum_variant_names)]
pub enum PipelineCommand {
    StartTopicMetadata,   // Phase 2: Pull missing configs
    StartTopicValidation, // Phase 2.5: Dual-hash check
    StartMessages,        // Phase 3: Message diff
}

pub struct SyncPipeline {
    command_tx: mpsc::UnboundedSender<PipelineCommand>,
}

impl SyncPipeline {
    pub fn new(command_tx: mpsc::UnboundedSender<PipelineCommand>) -> Self {
        Self { command_tx }
    }

    /// 进入 Phase 2: Topic 元数据补全
    pub fn on_owner_metadata_done(&self) -> Result<(), String> {
        self.command_tx
            .send(PipelineCommand::StartTopicMetadata)
            .map_err(|_| "sync pipeline command receiver closed".to_string())
    }

    /// 进入 Phase 2.5: Topic 哈希比对
    pub fn on_topic_metadata_pull_done(&self) -> Result<(), String> {
        self.command_tx
            .send(PipelineCommand::StartTopicValidation)
            .map_err(|_| "sync pipeline command receiver closed".to_string())
    }

    /// 进入 Phase 3: 消息同步
    pub fn on_topic_validation_done(&self) -> Result<(), String> {
        self.command_tx
            .send(PipelineCommand::StartMessages)
            .map_err(|_| "sync pipeline command receiver closed".to_string())
    }
}
