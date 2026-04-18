use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PipelinePhase {
    Idle,
    Phase1Metadata { progress: PhaseProgress },
    Phase2Topic { progress: PhaseProgress },
    Phase3Message { progress: PhaseProgress },
    Completed,
    Failed { error: String, phase: String },
}

impl Default for PipelinePhase {
    fn default() -> Self {
        PipelinePhase::Idle
    }
}
