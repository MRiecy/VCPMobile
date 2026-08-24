use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const WIRE_ERROR_MARKER: &str = "SYNC_WIRE_ERROR:";
const MAX_ERROR_MESSAGE_CHARS: usize = 1024;
const MAX_FAILED_TOPIC_IDS: usize = 8;
const MAX_TOPIC_ID_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorOrigin {
    MobileUi,
    MobileNative,
    MobileSync,
    DesktopPlugin,
    DesktopCds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorStage {
    Preflight,
    Startup,
    Connect,
    Handshake,
    OwnerMetadata,
    TopicMetadata,
    TopicValidation,
    Messages,
    Finalize,
    Shutdown,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorCategory {
    Device,
    Configuration,
    Connection,
    Compatibility,
    Protocol,
    Data,
    Storage,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRetryAction {
    Automatic,
    AfterUserAction,
    Manual,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireSyncError {
    pub code: String,
    pub origin: SyncErrorOrigin,
    pub stage: SyncErrorStage,
    pub kind: SyncErrorCategory,
    pub retry: SyncRetryAction,
    pub message: String,
    pub failed_topic_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncErrorPayload {
    pub code: String,
    pub category: SyncErrorCategory,
    pub origin: SyncErrorOrigin,
    pub stage: SyncErrorStage,
    pub retry_action: SyncRetryAction,
    pub message: String,
    pub guidance: String,
    pub failed_topic_ids: Vec<String>,
    pub log_file: Option<String>,
}

#[derive(Clone, Copy)]
struct ErrorDefinition {
    category: SyncErrorCategory,
    origin: SyncErrorOrigin,
    stage: SyncErrorStage,
    retry: SyncRetryAction,
    message: &'static str,
    guidance: &'static str,
}

const fn definition(
    category: SyncErrorCategory,
    origin: SyncErrorOrigin,
    stage: SyncErrorStage,
    retry: SyncRetryAction,
    message: &'static str,
    guidance: &'static str,
) -> ErrorDefinition {
    ErrorDefinition {
        category,
        origin,
        stage,
        retry,
        message,
        guidance,
    }
}

fn error_definition(code: &str) -> Option<ErrorDefinition> {
    use SyncErrorCategory as Category;
    use SyncErrorOrigin as Origin;
    use SyncErrorStage as Stage;
    use SyncRetryAction as Retry;

    let value = match code {
        "POWER_SAVE_MODE" => definition(
            Category::Device,
            Origin::MobileNative,
            Stage::Preflight,
            Retry::AfterUserAction,
            "系统省电模式已阻止本次同步",
            "关闭系统省电模式后再试。",
        ),
        "BATTERY_TOO_LOW" => definition(
            Category::Device,
            Origin::MobileNative,
            Stage::Preflight,
            Retry::AfterUserAction,
            "当前电量不足，已暂停同步",
            "电量达到 30% 后再试。",
        ),
        "SYNC_ACTIVE_GENERATION" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::Preflight,
            Retry::AfterUserAction,
            "仍有消息正在生成，暂不能开始同步",
            "等待生成结束或手动停止生成后再试。",
        ),
        "CONFIG_LOOPBACK_ON_MOBILE" => definition(
            Category::Configuration,
            Origin::MobileSync,
            Stage::Preflight,
            Retry::AfterUserAction,
            "服务器地址仍指向本机，手机无法连接电脑",
            "在同步设置中填写电脑的局域网 IP 和端口。",
        ),
        "TOKEN_MISMATCH" | "SYNC_AUTH_FAILED" => definition(
            Category::Configuration,
            Origin::MobileSync,
            Stage::Connect,
            Retry::AfterUserAction,
            "手机端与电脑端的同步令牌不一致",
            "重新核对两端令牌后再试。",
        ),
        "SYNC_CONFIG_MISSING" | "SYNC_SETTINGS_READ_FAILED" => definition(
            Category::Configuration,
            Origin::MobileSync,
            Stage::Startup,
            Retry::AfterUserAction,
            "同步服务器地址尚未配置完整",
            "在同步设置中填写电脑端 WebSocket 和 HTTP 地址后再试。",
        ),
        "INVALID_CONFIGURATION" => definition(
            Category::Configuration,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::AfterUserAction,
            "电脑端数据服务配置无效",
            "检查并重启电脑端应用；若仍失败，请保留最新日志。",
        ),
        "UNAUTHORIZED" => definition(
            Category::Configuration,
            Origin::DesktopCds,
            Stage::Connect,
            Retry::AfterUserAction,
            "电脑端数据服务认证失败",
            "重启电脑端应用并检查其数据服务配置后再试。",
        ),
        "SYNC_CONFIG_INVALID" | "WS_PATH_INVALID" => definition(
            Category::Configuration,
            Origin::MobileSync,
            Stage::Connect,
            Retry::AfterUserAction,
            "同步服务器地址格式或路径不正确",
            "检查同步设置中的协议、IP、端口和服务路径后再试。",
        ),
        "SYNC_CHANGE_FEED_UNAVAILABLE" => definition(
            Category::Configuration,
            Origin::DesktopPlugin,
            Stage::History,
            Retry::AfterUserAction,
            "电脑端未启用同步历史能力",
            "在电脑端启用中央同步服务后再试。",
        ),
        "SYNC_VERSION_INCOMPATIBLE" | "PROTOCOL_MISMATCH" | "PLUGIN_VERSION_MISMATCH" => {
            definition(
                Category::Compatibility,
                Origin::DesktopPlugin,
                Stage::Handshake,
                Retry::AfterUserAction,
                "手机端与电脑端同步版本不兼容",
                "将手机端与电脑端同步插件更新到同一兼容版本后再试。",
            )
        }
        "CDS_PROTOCOL_MISMATCH" => definition(
            Category::Compatibility,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::AfterUserAction,
            "电脑端内部数据服务版本不兼容",
            "更新并重启电脑端应用后重新同步。",
        ),
        "VERSION_CHECK_TIMEOUT" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Handshake,
            Retry::Manual,
            "电脑端未在规定时间内完成版本握手",
            "确认电脑端同步插件已启动且两端网络正常，然后重新同步。",
        ),
        "MANIFEST_RESPONSE_TIMEOUT" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::OwnerMetadata,
            Retry::Manual,
            "电脑端未在规定时间内返回所有者数据",
            "确认电脑端数据服务正常，然后重新同步。",
        ),
        "TOPIC_HASH_RESPONSE_TIMEOUT" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::TopicValidation,
            Retry::Manual,
            "电脑端未在规定时间内返回话题校验结果",
            "确认电脑端数据服务正常，然后重新同步。",
        ),
        "FINAL_ACK_TIMEOUT" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Finalize,
            Retry::Manual,
            "电脑端未确认同步收尾，系统未将本次任务标记为成功",
            "确认电脑端服务正常，然后重新同步。",
        ),
        "CONNECTION_REFUSED" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Connect,
            Retry::Manual,
            "端口未开放：可能是没启动服务插件或正在构建索引",
            "请稍后重新同步。",
        ),
        "WS_CLOSED"
        | "WS_DISCONNECTED"
        | "WS_RECEIVE_FAILED"
        | "WS_SEND_FAILED"
        | "HTTP_HANDSHAKE_REJECTED" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Connect,
            Retry::Manual,
            "同步通道中断或响应超时",
            "确认两端处于同一网络且电脑端服务正常，然后重新同步。",
        ),
        "TIMEOUT" | "UNAVAILABLE" | "HTTP_ERROR" | "HEALTH_CHECK_FAILED" => definition(
            Category::Connection,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::Manual,
            "电脑端数据服务连接异常或响应超时",
            "重启电脑端应用后重新同步；若仍失败，请保留最新日志。",
        ),
        "SYNC_STREAM_FAILED" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Messages,
            Retry::Manual,
            "消息同步流意外中断",
            "确认两端网络与电脑端服务正常，然后重新同步。",
        ),
        "SERVICE_BUSY" => definition(
            Category::Connection,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::Manual,
            "电脑端数据服务持续繁忙",
            "等待电脑端当前任务结束后重新同步。",
        ),
        "VCP_LOG_DISCONNECTED" => definition(
            Category::Connection,
            Origin::MobileSync,
            Stage::Preflight,
            Retry::Manual,
            "尚未连接电脑端服务通道",
            "确认电脑端服务已启动，并等待连接成功后再试。",
        ),
        "PROTOCOL_INVALID"
        | "PROTOCOL_DUPLICATE_KEY"
        | "VERSION_CHECK_REQUIRED"
        | "VERSION_CHECK_DUPLICATE"
        | "VERSION_CHECK_INVALID"
        | "VERSION_ACK_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::Handshake,
            Retry::AfterUserAction,
            "同步响应不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
        ),
        "INVALID_RESPONSE" | "INVALID_REQUEST" => definition(
            Category::Protocol,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::AfterUserAction,
            "电脑端数据服务响应不符合预期",
            "更新或重启电脑端应用后重新同步；若仍出现，请保留最新日志。",
        ),
        "SYNC_PROTOCOL_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::OwnerMetadata,
            Retry::AfterUserAction,
            "所有者同步响应不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
        ),
        "SYNC_REQUEST_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::Connect,
            Retry::AfterUserAction,
            "同步请求不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重新同步；若仍出现，请保留最新日志。",
        ),
        "PROTOCOL_FRAME_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::Messages,
            Retry::AfterUserAction,
            "运行中的同步消息不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
        ),
        "TOPIC_HASH_RESPONSE_OVERLAP" | "TOPIC_HASH_RESULTS_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::TopicValidation,
            Retry::AfterUserAction,
            "话题校验响应不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
        ),
        "PHASE3_BATCH_OVERLAP"
        | "PHASE3_FRAME_INVALID"
        | "PHASE3_DECISION_INVALID"
        | "PHASE3_TOPIC_MISMATCH"
        | "SYNC_DELETE_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::Messages,
            Retry::AfterUserAction,
            "消息同步响应不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
        ),
        "SYNC_LOG_PATH_INVALID" => definition(
            Category::Protocol,
            Origin::MobileSync,
            Stage::History,
            Retry::Never,
            "日志文件标识不合法，已拒绝访问",
            "返回日志列表后重新选择文件。",
        ),
        "OWNER_MANIFEST_INVALID" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::OwnerMetadata,
            Retry::Manual,
            "手机端所有者数据不完整或相互冲突",
            "检查手机端智能体与群组数据后重新同步；若仍失败，请保留日志。",
        ),
        "TOPIC_MANIFEST_INVALID" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::TopicMetadata,
            Retry::Manual,
            "手机端话题数据不完整或归属冲突",
            "检查手机端话题数据后重新同步；若仍失败，请保留日志。",
        ),
        "SYNC_BUDGET_EXCEEDED"
        | "PHASE3_DIFF_BUDGET_EXCEEDED"
        | "PHASE3_DECISION_BUDGET_EXCEEDED"
        | "RESPONSE_TOO_LARGE" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::Messages,
            Retry::AfterUserAction,
            "本次消息同步数据量超过安全上限",
            "拆分或清理异常大的会话后再试。",
        ),
        "TOPIC_HASH_BUDGET_EXCEEDED" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::TopicValidation,
            Retry::AfterUserAction,
            "本次同步数据量超过安全上限",
            "拆分或清理异常大的会话后再试。",
        ),
        "SYNC_OWNER_CONFLICT" => definition(
            Category::Data,
            Origin::DesktopPlugin,
            Stage::TopicMetadata,
            Retry::Manual,
            "同步数据的归属关系发生冲突，已安全停止",
            "在电脑端检查重复或归属冲突的话题，处理后重新同步。",
        ),
        "TOPIC_PUSH_OWNER_CONFLICT" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::TopicMetadata,
            Retry::Manual,
            "待上传话题的归属关系已变化，系统已安全停止",
            "检查手机端对应话题的归属关系后重新同步。",
        ),
        "SYNC_ENTITY_NOT_FOUND" | "SYNC_AVATAR_NOT_FOUND" => definition(
            Category::Data,
            Origin::DesktopPlugin,
            Stage::OwnerMetadata,
            Retry::Manual,
            "部分所有者数据缺失，系统未将其标记为成功",
            "先在电脑端检查对应智能体、群组或头像，处理后重新同步。",
        ),
        "TOPIC_PUSH_SOURCE_MISSING" => definition(
            Category::Data,
            Origin::MobileSync,
            Stage::TopicMetadata,
            Retry::Manual,
            "待上传的话题已不存在，系统已安全停止",
            "检查手机端对应话题后重新同步。",
        ),
        "HISTORY_SOURCE_INVALID" | "MOBILE_ATTACHMENT_INVALID" => definition(
            Category::Data,
            Origin::DesktopPlugin,
            Stage::Messages,
            Retry::AfterUserAction,
            "部分同步来源数据不完整或已损坏",
            "先在电脑端检查对应会话或附件，处理后重新同步。",
        ),
        "ATTACHMENT_PATH_INVALID" => definition(
            Category::Storage,
            Origin::DesktopPlugin,
            Stage::Messages,
            Retry::AfterUserAction,
            "电脑端附件路径无效，无法读取文件",
            "在电脑端重新选择或移除对应附件后再同步。",
        ),
        "TOPIC_NOT_FOUND" | "PHASE3_DIFF_MISSING" | "NOT_FOUND" | "AMBIGUOUS_IDENTITY" => {
            definition(
                Category::Data,
                Origin::DesktopPlugin,
                Stage::Messages,
                Retry::Manual,
                "部分同步数据缺失或已损坏，系统未将其标记为成功",
                "先在电脑端检查对应会话或附件，处理后重新同步。",
            )
        }
        "SYNC_DB_UNAVAILABLE" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::Startup,
            Retry::Manual,
            "同步数据库当前不可用",
            "重启应用后重新同步；若仍失败，请检查可用存储空间并保留日志。",
        ),
        "SYNC_ENTITY_READ_FAILED"
        | "SYNC_ENTITY_WRITE_FAILED"
        | "SYNC_AVATAR_READ_FAILED"
        | "SYNC_AVATAR_WRITE_FAILED"
        | "OWNER_MANIFEST_DB_FAILED"
        | "AVATAR_MANIFEST_DB_FAILED"
        | "AGENT_HASH_INIT_DB_FAILED"
        | "GROUP_HASH_INIT_DB_FAILED"
        | "OWNER_METADATA_DRAIN_FAILED"
        | "ENTITY_PULL_FAILED" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::OwnerMetadata,
            Retry::Manual,
            "所有者数据读取或写入失败，系统未将其标记为成功",
            "检查手机与电脑端的存储和数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        "SYNC_ENTITY_BATCH_FAILED"
        | "TOPIC_MANIFEST_DB_FAILED"
        | "TOPIC_METADATA_DRAIN_FAILED"
        | "TOPIC_PUSH_DB_DECODE_FAILED"
        | "TOPIC_PUSH_DB_FAILED"
        | "TOPIC_PUSH_FAILED" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::TopicMetadata,
            Retry::Manual,
            "话题数据读取或写入失败，系统未将其标记为成功",
            "检查手机与电脑端的存储和数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        "SYNC_DB_QUERY_FAILED"
        | "SYNC_INDEX_INVALID"
        | "TOPIC_HASH_DB_FAILED"
        | "TOPIC_VALIDATION_DRAIN_FAILED" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::TopicValidation,
            Retry::Manual,
            "话题校验数据读取失败，系统未将其标记为成功",
            "检查手机与电脑端的数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        "MESSAGE_DIFF_FAILED"
        | "TOPIC_HASH_FAILED"
        | "MESSAGE_MANIFEST_FAILED"
        | "SYNC_MESSAGE_READ_FAILED"
        | "SYNC_MESSAGE_WRITE_FAILED"
        | "SYNC_DELETE_FAILED"
        | "ENTITY_UPDATE_FAILED"
        | "ENTITY_DELETE_FAILED"
        | "ENTITY_OPERATION_FAILED"
        | "PHASE3_HASH_PREP_FAILED"
        | "PHASE3_PUSH_FAILED"
        | "PHASE3_PULL_FAILED" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::Messages,
            Retry::Manual,
            "同步数据读取或写入失败，系统未将其标记为成功",
            "检查手机与电脑端的存储和数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        "FINAL_WRITE_DRAIN_FAILED"
        | "RETRY_WRITE_DRAIN_FAILED"
        | "SYNC_DB_DRAIN_FAILED"
        | "SYNC_FINALIZATION_FAILED" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::Finalize,
            Retry::Manual,
            "同步收尾写入未完成，系统未将本次任务标记为成功",
            "确认两端存储和数据服务正常后重新同步；若仍失败，请保留日志。",
        ),
        "SYNC_CHANGE_FEED_FAILED"
        | "SYNC_LOG_LIST_FAILED"
        | "SYNC_LOG_READ_FAILED"
        | "SYNC_LOG_CLEAR_FAILED"
        | "SEARCH_UNAVAILABLE" => definition(
            Category::Storage,
            Origin::MobileSync,
            Stage::History,
            Retry::Manual,
            "同步历史或日志读取失败",
            "检查可用存储空间后重试；若仍失败，请保留日志。",
        ),
        "CDS_UNAVAILABLE" | "CDS_ERROR" | "INTERNAL_ERROR" => definition(
            Category::Internal,
            Origin::DesktopCds,
            Stage::Startup,
            Retry::Manual,
            "电脑端数据服务未能启动",
            "重启电脑端应用后重新同步；若仍失败，请保留日志。",
        ),
        "SYNC_ALREADY_RUNNING" => definition(
            Category::Internal,
            Origin::MobileSync,
            Stage::Startup,
            Retry::Never,
            "已有同步任务正在运行",
            "请等待当前同步结束。",
        ),
        "CANCELLED" => definition(
            Category::Internal,
            Origin::DesktopCds,
            Stage::Shutdown,
            Retry::Never,
            "本次同步请求已取消",
            "无需重试；需要同步时重新开始新任务。",
        ),
        "SYNC_STOP_FAILED" | "SYNC_PREVIOUS_SESSION_EXIT_FAILED" => definition(
            Category::Internal,
            Origin::MobileSync,
            Stage::Shutdown,
            Retry::Manual,
            "上一同步任务未能正常退出",
            "重启应用后重新同步；若仍失败，请保留最新日志。",
        ),
        "MOBILE_SYNC_ERROR" => definition(
            Category::Internal,
            Origin::MobileSync,
            Stage::Shutdown,
            Retry::Manual,
            "手机端同步核心已终止本次任务",
            "重新同步；若再次出现，请保留手机与电脑端最新日志。",
        ),
        "REMOTE_SYNC_FAILED" => definition(
            Category::Internal,
            Origin::DesktopPlugin,
            Stage::Connect,
            Retry::Manual,
            "电脑端同步组件未能完成本次任务",
            "重启电脑端应用后重新同步；若仍失败，请保留最新日志。",
        ),
        "SYNC_STATE_POISONED"
        | "SYNC_START_CHANNEL_FAILED"
        | "SYNC_DIFF_HANDLER_FAILED"
        | "SYNC_ATTEMPT_FAILED"
        | "HTTP_CLIENT_INIT_FAILED" => definition(
            Category::Internal,
            Origin::MobileSync,
            Stage::Startup,
            Retry::Manual,
            "同步组件未能正常完成本次任务",
            "重启应用后重新同步；若仍失败，请保留最新日志。",
        ),
        _ => return None,
    };
    Some(value)
}

fn fallback_copy(category: SyncErrorCategory) -> (&'static str, &'static str) {
    match category {
        SyncErrorCategory::Device => ("设备状态暂不满足同步条件", "按系统提示调整设备状态后再试。"),
        SyncErrorCategory::Configuration => {
            ("同步配置需要调整", "检查两端地址、端口和令牌后再试。")
        }
        SyncErrorCategory::Connection => (
            "同步通道中断或响应超时",
            "确认网络与电脑端服务正常后重新同步。",
        ),
        SyncErrorCategory::Compatibility => (
            "手机端与电脑端同步版本不兼容",
            "将两端更新到同一兼容版本后再试。",
        ),
        SyncErrorCategory::Protocol => (
            "同步响应不符合 Wire 1.3 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留日志。",
        ),
        SyncErrorCategory::Data => (
            "部分同步数据缺失、冲突或无法处理",
            "检查电脑端对应数据后重新同步；若仍失败，请保留日志。",
        ),
        SyncErrorCategory::Storage => (
            "同步数据读取或写入失败",
            "检查两端存储与数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        SyncErrorCategory::Internal => (
            "同步组件未能正常完成本次任务",
            "重启应用后重新同步；若仍失败，请保留最新日志。",
        ),
    }
}

fn is_valid_wire_code(code: &str) -> bool {
    if code.is_empty() || code.len() > 64 {
        return false;
    }
    let mut bytes = code.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_uppercase())
        || !bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return false;
    }
    let platform_error = matches!(
        code,
        "E2BIG"
            | "EACCES"
            | "EADDRINUSE"
            | "EADDRNOTAVAIL"
            | "EAGAIN"
            | "EBADF"
            | "EBUSY"
            | "ECONNABORTED"
            | "ECONNREFUSED"
            | "ECONNRESET"
            | "EEXIST"
            | "EFAULT"
            | "EHOSTUNREACH"
            | "EINTR"
            | "EINVAL"
            | "EIO"
            | "EISDIR"
            | "ELOOP"
            | "EMFILE"
            | "EMSGSIZE"
            | "ENAMETOOLONG"
            | "ENETDOWN"
            | "ENETUNREACH"
            | "ENFILE"
            | "ENOBUFS"
            | "ENODEV"
            | "ENOENT"
            | "ENOMEM"
            | "ENOSPC"
            | "ENOTDIR"
            | "ENOTEMPTY"
            | "ENOTFOUND"
            | "ENOTSUP"
            | "EPERM"
            | "EPIPE"
            | "EROFS"
            | "ETIMEDOUT"
    );
    !platform_error
        && !code.starts_with("EAI_")
        && !code.starts_with("ERR_")
        && !code.starts_with("SQLITE_")
}

fn sanitize_topic_ids<I>(topic_ids: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    topic_ids
        .into_iter()
        .filter(|id| {
            !id.is_empty() && id.chars().count() <= MAX_TOPIC_ID_CHARS && seen.insert(id.clone())
        })
        .take(MAX_FAILED_TOPIC_IDS)
        .collect()
}

fn validate_wire_error(error: WireSyncError) -> Result<WireSyncError, String> {
    if !is_valid_wire_code(&error.code) {
        return Err("error.code is invalid".to_string());
    }
    if error.message.trim().is_empty() || error.message.chars().count() > MAX_ERROR_MESSAGE_CHARS {
        return Err("error.message is invalid".to_string());
    }
    if error.failed_topic_ids.len() > MAX_FAILED_TOPIC_IDS
        || error
            .failed_topic_ids
            .iter()
            .any(|id| id.is_empty() || id.chars().count() > MAX_TOPIC_ID_CHARS)
        || error.failed_topic_ids.iter().collect::<HashSet<_>>().len()
            != error.failed_topic_ids.len()
    {
        return Err("error.failedTopicIds is invalid".to_string());
    }
    if let Some(registered) = error_definition(&error.code) {
        if error.kind != registered.category || error.retry != registered.retry {
            return Err("error.kind or error.retry conflicts with its registered code".to_string());
        }
    }
    Ok(error)
}

pub fn parse_wire_sync_error(value: &Value) -> Result<WireSyncError, String> {
    let error = serde_json::from_value::<WireSyncError>(value.clone())
        .map_err(|parse_error| format!("invalid Wire 1.3 error object: {parse_error}"))?;
    validate_wire_error(error)
}

pub fn encode_wire_sync_error(error: &WireSyncError) -> Result<String, String> {
    let validated = validate_wire_error(error.clone())?;
    serde_json::to_string(&validated)
        .map(|json| format!("{WIRE_ERROR_MARKER}{json}"))
        .map_err(|serialize_error| format!("failed to encode Wire 1.3 error: {serialize_error}"))
}

pub fn encode_wire_sync_error_value(value: &Value) -> Result<String, String> {
    encode_wire_sync_error(&parse_wire_sync_error(value)?)
}

pub fn encode_http_sync_error_body(bytes: &[u8]) -> Result<Option<String>, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("HTTP error body is not valid JSON: {error}"))?;
    match value.get("error") {
        Some(error) => encode_wire_sync_error_value(error).map(Some),
        None => Ok(None),
    }
}

pub fn decode_wire_sync_error(text: &str) -> Option<WireSyncError> {
    let marker_offset = text.find(WIRE_ERROR_MARKER)? + WIRE_ERROR_MARKER.len();
    let mut stream =
        serde_json::Deserializer::from_str(&text[marker_offset..]).into_iter::<WireSyncError>();
    validate_wire_error(stream.next()?.ok()?).ok()
}

pub fn build_local_error_payload(
    code: &str,
    failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    let stable_code = if is_valid_wire_code(code) {
        code
    } else {
        "SYNC_ATTEMPT_FAILED"
    };
    let fallback = error_definition("SYNC_ATTEMPT_FAILED").expect("fallback definition");
    let selected = error_definition(stable_code).unwrap_or(fallback);
    SyncErrorPayload {
        code: stable_code.to_string(),
        category: selected.category,
        origin: selected.origin,
        stage: selected.stage,
        retry_action: selected.retry,
        message: selected.message.to_string(),
        guidance: selected.guidance.to_string(),
        failed_topic_ids: sanitize_topic_ids(failed_topic_ids),
        log_file,
    }
}

pub fn build_wire_error_payload(
    wire: &WireSyncError,
    additional_failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    let (message, guidance) = error_definition(&wire.code)
        .map(|known| (known.message, known.guidance))
        .unwrap_or_else(|| fallback_copy(wire.kind));
    let failed_topic_ids = wire
        .failed_topic_ids
        .iter()
        .cloned()
        .chain(additional_failed_topic_ids)
        .collect::<Vec<_>>();
    SyncErrorPayload {
        code: wire.code.clone(),
        category: wire.kind,
        origin: wire.origin,
        stage: wire.stage,
        retry_action: wire.retry,
        message: message.to_string(),
        guidance: guidance.to_string(),
        failed_topic_ids: sanitize_topic_ids(failed_topic_ids),
        log_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_wire_errors_are_strict_and_match_the_local_registry() {
        let bytes = include_bytes!("fixtures/error_contract_1_2_golden.json");
        let fixture: Value = serde_json::from_slice(bytes).expect("golden error fixture");
        for entry in fixture["validErrors"].as_array().expect("validErrors") {
            parse_wire_sync_error(&entry["error"]).expect("valid wire error");
        }
        for entry in fixture["invalidErrors"].as_array().expect("invalidErrors") {
            assert!(parse_wire_sync_error(&entry["error"]).is_err());
        }
        for (code, semantics) in fixture["registeredSemantics"]
            .as_object()
            .expect("registeredSemantics")
        {
            let semantics = semantics.as_array().expect("semantic tuple");
            let registered = error_definition(code)
                .unwrap_or_else(|| panic!("missing registered semantics for {code}"));
            assert_eq!(
                serde_json::to_value(registered.category).expect("serialize category"),
                semantics[0],
                "kind for {code}"
            );
            assert_eq!(
                serde_json::to_value(registered.retry).expect("serialize retry"),
                semantics[1],
                "retry for {code}"
            );
        }
    }

    #[test]
    fn exact_registry_does_not_confuse_version_protocol_device_and_storage() {
        let cases = [
            (
                "POWER_SAVE_MODE",
                SyncErrorCategory::Device,
                SyncErrorOrigin::MobileNative,
                SyncErrorStage::Preflight,
            ),
            (
                "VERSION_ACK_INVALID",
                SyncErrorCategory::Protocol,
                SyncErrorOrigin::MobileSync,
                SyncErrorStage::Handshake,
            ),
            (
                "SYNC_VERSION_INCOMPATIBLE",
                SyncErrorCategory::Compatibility,
                SyncErrorOrigin::DesktopPlugin,
                SyncErrorStage::Handshake,
            ),
            (
                "OWNER_MANIFEST_INVALID",
                SyncErrorCategory::Data,
                SyncErrorOrigin::MobileSync,
                SyncErrorStage::OwnerMetadata,
            ),
            (
                "SYNC_DB_DRAIN_FAILED",
                SyncErrorCategory::Storage,
                SyncErrorOrigin::MobileSync,
                SyncErrorStage::Finalize,
            ),
            (
                "INVALID_CONFIGURATION",
                SyncErrorCategory::Configuration,
                SyncErrorOrigin::DesktopCds,
                SyncErrorStage::Startup,
            ),
            (
                "UNAUTHORIZED",
                SyncErrorCategory::Configuration,
                SyncErrorOrigin::DesktopCds,
                SyncErrorStage::Connect,
            ),
            (
                "TIMEOUT",
                SyncErrorCategory::Connection,
                SyncErrorOrigin::DesktopCds,
                SyncErrorStage::Startup,
            ),
        ];

        for (code, category, origin, stage) in cases {
            let payload = build_local_error_payload(code, Vec::new(), None);
            assert_eq!(payload.category, category, "category for {code}");
            assert_eq!(payload.origin, origin, "origin for {code}");
            assert_eq!(payload.stage, stage, "stage for {code}");
        }

        let cds_auth = build_local_error_payload("UNAUTHORIZED", Vec::new(), None);
        assert!(!cds_auth.guidance.contains("两端令牌"));
        let cds_timeout = build_local_error_payload("TIMEOUT", Vec::new(), None);
        assert!(!cds_timeout.guidance.contains("同一网络"));
    }

    #[test]
    fn timeout_codes_keep_the_phase_that_timed_out() {
        let cases = [
            ("VERSION_CHECK_TIMEOUT", SyncErrorStage::Handshake),
            ("MANIFEST_RESPONSE_TIMEOUT", SyncErrorStage::OwnerMetadata),
            (
                "TOPIC_HASH_RESPONSE_TIMEOUT",
                SyncErrorStage::TopicValidation,
            ),
            ("FINAL_ACK_TIMEOUT", SyncErrorStage::Finalize),
        ];

        for (code, stage) in cases {
            let payload = build_local_error_payload(code, Vec::new(), None);
            assert_eq!(payload.category, SyncErrorCategory::Connection);
            assert_eq!(payload.stage, stage, "stage for {code}");
        }
    }

    #[test]
    fn unknown_wire_code_keeps_metadata_but_never_exposes_raw_message() {
        let wire = parse_wire_sync_error(&serde_json::json!({
            "code": "UPSTREAM_EXTENSION_FAILED",
            "origin": "desktop_plugin",
            "stage": "finalize",
            "kind": "internal",
            "retry": "manual",
            "message": "Bearer desktop-secret",
            "failedTopicIds": []
        }))
        .expect("unknown stable code");
        let payload = build_wire_error_payload(&wire, Vec::new(), None);
        assert_eq!(payload.code, "UPSTREAM_EXTENSION_FAILED");
        assert_eq!(payload.stage, SyncErrorStage::Finalize);
        assert!(!payload.message.contains("desktop-secret"));
    }

    #[test]
    fn encoded_wire_error_survives_aggregate_diagnostics() {
        let wire = parse_wire_sync_error(&serde_json::json!({
            "code": "SYNC_OWNER_CONFLICT",
            "origin": "desktop_cds",
            "stage": "messages",
            "kind": "data",
            "retry": "manual",
            "message": "owner conflict",
            "failedTopicIds": ["topic-a"]
        }))
        .expect("wire error");
        let encoded = encode_wire_sync_error(&wire).expect("encode wire error");
        let aggregate = format!("topic-a: {encoded}, topic-b: local failure");
        assert_eq!(decode_wire_sync_error(&aggregate), Some(wire));
    }

    #[test]
    fn http_error_body_uses_the_same_wire_object() {
        let bytes = br#"{
            "error": {
                "code": "SYNC_DB_QUERY_FAILED",
                "origin": "desktop_cds",
                "stage": "messages",
                "kind": "storage",
                "retry": "manual",
                "message": "query failed",
                "failedTopicIds": ["topic-a"]
            }
        }"#;
        let encoded = encode_http_sync_error_body(bytes)
            .expect("HTTP error body")
            .expect("error object");
        let decoded = decode_wire_sync_error(&encoded).expect("encoded HTTP error");
        assert_eq!(decoded.code, "SYNC_DB_QUERY_FAILED");
        assert_eq!(decoded.origin, SyncErrorOrigin::DesktopCds);
    }

    #[test]
    fn platform_errno_is_not_promoted_to_a_wire_code() {
        let payload = build_local_error_payload("ENOENT", Vec::new(), None);
        assert_eq!(payload.code, "SYNC_ATTEMPT_FAILED");
        let stable = build_local_error_payload("EXTENSIONFAILED", Vec::new(), None);
        assert_eq!(stable.code, "EXTENSIONFAILED");
    }

    #[test]
    fn known_wire_code_cannot_claim_a_different_category() {
        let error = parse_wire_sync_error(&serde_json::json!({
            "code": "POWER_SAVE_MODE",
            "origin": "mobile_native",
            "stage": "preflight",
            "kind": "compatibility",
            "retry": "after_user_action",
            "message": "wrong category",
            "failedTopicIds": []
        }))
        .expect_err("known code category mismatch");
        assert!(error.contains("conflicts with its registered code"));
    }

    #[test]
    fn wire_bounds_count_unicode_scalar_values() {
        let valid = serde_json::json!({
            "code": "UPSTREAM_EXTENSION_FAILED",
            "origin": "desktop_plugin",
            "stage": "messages",
            "kind": "internal",
            "retry": "manual",
            "message": "🙂".repeat(1024),
            "failedTopicIds": ["🙂".repeat(512)]
        });
        parse_wire_sync_error(&valid).expect("Unicode scalar boundary");

        let mut invalid = valid;
        invalid["message"] = Value::String("🙂".repeat(1025));
        assert!(parse_wire_sync_error(&invalid).is_err());
    }
}
