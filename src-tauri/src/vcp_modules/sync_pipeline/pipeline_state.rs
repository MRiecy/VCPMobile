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

    pub fn with_items(items: Vec<String>) -> Self {
        let total = items.len() as u32;
        Self {
            total,
            completed: 0,
            pending: items,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.completed >= self.total
    }

    pub fn complete_one(&mut self) {
        self.completed += 1;
        if !self.pending.is_empty() {
            self.pending.remove(0);
        }
    }

    pub fn percentage(&self) -> f32 {
        if self.total == 0 {
            100.0
        } else {
            (self.completed as f32 / self.total as f32) * 100.0
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

impl PipelinePhase {
    pub fn phase_name(&self) -> &str {
        match self {
            PipelinePhase::Idle => "idle",
            PipelinePhase::Phase1Metadata { .. } => "phase1_metadata",
            PipelinePhase::Phase2Topic { .. } => "phase2_topic",
            PipelinePhase::Phase3Message { .. } => "phase3_message",
            PipelinePhase::Completed => "completed",
            PipelinePhase::Failed { .. } => "failed",
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self,
            PipelinePhase::Phase1Metadata { .. }
                | PipelinePhase::Phase2Topic { .. }
                | PipelinePhase::Phase3Message { .. }
        )
    }

    pub fn progress(&self) -> Option<&PhaseProgress> {
        match self {
            PipelinePhase::Phase1Metadata { progress } => Some(progress),
            PipelinePhase::Phase2Topic { progress } => Some(progress),
            PipelinePhase::Phase3Message { progress } => Some(progress),
            _ => None,
        }
    }
}

impl Default for PipelinePhase {
    fn default() -> Self {
        PipelinePhase::Idle
    }
}
