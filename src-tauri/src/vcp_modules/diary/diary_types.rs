use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiaryErrorCode {
    ConfigMissing,
    InvalidRequest,
    AuthRequired,
    Forbidden,
    NotFound,
    RateLimited,
    Conflict,
    Cancelled,
    Timeout,
    Transport,
    ResponseTooLarge,
    InvalidResponse,
    ServiceUnavailable,
    ServerError,
    SaveUncertain,
    CreateUncertain,
    ToolError,
}

impl DiaryErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigMissing => "DIARY_CONFIG_MISSING",
            Self::InvalidRequest => "DIARY_INVALID_REQUEST",
            Self::AuthRequired => "DIARY_AUTH_REQUIRED",
            Self::Forbidden => "DIARY_FORBIDDEN",
            Self::NotFound => "DIARY_NOT_FOUND",
            Self::RateLimited => "DIARY_RATE_LIMITED",
            Self::Conflict => "DIARY_CONFLICT",
            Self::Cancelled => "DIARY_CANCELLED",
            Self::Timeout => "DIARY_TIMEOUT",
            Self::Transport => "DIARY_TRANSPORT",
            Self::ResponseTooLarge => "DIARY_RESPONSE_TOO_LARGE",
            Self::InvalidResponse => "DIARY_INVALID_RESPONSE",
            Self::ServiceUnavailable => "DIARY_SERVICE_UNAVAILABLE",
            Self::ServerError => "DIARY_SERVER_ERROR",
            Self::SaveUncertain => "DIARY_SAVE_UNCERTAIN",
            Self::CreateUncertain => "DIARY_CREATE_UNCERTAIN",
            Self::ToolError => "DIARY_TOOL_ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryError {
    pub code: DiaryErrorCode,
    pub message: String,
}

impl DiaryError {
    pub fn new(code: DiaryErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn command_string(&self) -> String {
        format!("{}: {}", self.code.as_str(), self.message)
    }
}

impl fmt::Display for DiaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for DiaryError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct DiaryNoteKey {
    pub folder: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryFolderList {
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryNoteSummary {
    pub folder: String,
    pub file: String,
    pub last_modified: String,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryDocument {
    pub key: DiaryNoteKey,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySearchRequest {
    pub request_id: String,
    pub term: String,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySearchResponse {
    pub notes: Vec<DiaryNoteSummary>,
    pub total: usize,
    pub limited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryCancelRequest {
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySemanticSearchRequest {
    pub request_id: String,
    pub query: String,
    pub folder: Option<String>,
    pub search_all: bool,
    pub k: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySemanticHit {
    pub key: DiaryNoteKey,
    pub preview: String,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySemanticResponse {
    pub hits: Vec<DiarySemanticHit>,
    pub index_may_be_catching_up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySaveRequest {
    pub key: DiaryNoteKey,
    pub content: String,
    pub baseline_hash: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiarySaveOutcome {
    pub content_hash: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryRenameRequest {
    pub source: DiaryNoteKey,
    pub target_file: String,
    pub baseline_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiaryRenameStatus {
    Renamed,
    CopiedSourceRetained,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryRenameOutcome {
    pub key: DiaryNoteKey,
    pub content_hash: String,
    pub status: DiaryRenameStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryCreateRequest {
    pub maid: String,
    pub date: String,
    pub folder: Option<String>,
    pub file_name_suffix: Option<String>,
    pub tag: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiaryIndexStatus {
    Queued,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryCreateOutcome {
    pub key: DiaryNoteKey,
    pub index_status: DiaryIndexStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryMoveRequest {
    pub sources: Vec<DiaryNoteKey>,
    pub target_folder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryDeleteRequest {
    pub sources: Vec<DiaryNoteKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryBatchError {
    pub key: DiaryNoteKey,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryBatchOutcome {
    pub succeeded: Vec<DiaryNoteKey>,
    pub errors: Vec<DiaryBatchError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiaryDeleteFolderRequest {
    pub folder: String,
}
