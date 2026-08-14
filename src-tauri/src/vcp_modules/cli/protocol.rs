use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::manifest::VCP_MOBILE_CLI_TOOL_NAME;
use super::result::VcpCliErrorCode;

const TOOL_REQUEST_START: &str = "<<<[TOOL_REQUEST]>>>";
const TOOL_REQUEST_END: &str = "<<<[END_TOOL_REQUEST]>>>";
const TOOL_REQUEST_START_ESCAPE: &str = "<<<[TOOL_REQUEST_ESCAPE]>>>";
const TOOL_REQUEST_END_ESCAPE: &str = "<<<[END_TOOL_REQUEST_ESCAPE]>>>";
const FIELD_START: &str = "「始」";
const FIELD_END: &str = "「末」";

pub const DEFAULT_CWD: &str = "/workspace";
pub const DEFAULT_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
pub const MIN_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 12 * 60 * 60 * 1_000;
pub const DEFAULT_BOUNDED_READ_BYTES: usize = 65_536;
pub const MAX_BOUNDED_READ_BYTES: usize = 262_144;
pub const MAX_POLL_WAIT_MS: u64 = 8_000;
pub const MAX_COMMAND_BYTES: usize = 65_536;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_CURSOR_BYTES: usize = 512;
const MAX_RESOURCE_PATH_BYTES: usize = 4_096;

static REASONING_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<\s*(/?)\s*(think(?:ing)?)\b[^>]*>")
        .expect("reasoning tag regex is a static invariant")
});
static ESCAPE_FIELD_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[「{]始escape[」}]").expect("escape field start regex is a static invariant")
});
static ESCAPE_FIELD_START_ANYWHERE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[「{]始escape[」}]").expect("escape field search regex is a static invariant")
});
static ESCAPE_FIELD_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[「{]末escape[」}]").expect("escape field end regex is a static invariant")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawVcpField {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawVcpToolRequest {
    pub fields: Vec<RawVcpField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum VcpCliAction {
    Run {
        command: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_in_background: Option<bool>,
    },
    ListSkills,
    ReadSkill {
        skill_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
    },
    MaterializeSkill {
        skill_id: String,
    },
    Poll {
        job_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_bytes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    Cancel {
        job_id: String,
    },
    List,
}

/// UI/内部 adapter 的结构化 action 也必须经过与 Human Tool 相同的 validator。
/// 这里先还原规范字符串字段，再复用唯一 `validate_vcp_mobile_cli_request`；
/// 不接受 WebView 传入的 marker 文本。
pub fn validate_structured_vcp_cli_action(
    action: VcpCliAction,
) -> Result<VcpCliAction, VcpCliProtocolError> {
    let mut fields = vec![RawVcpField {
        key: "tool_name".to_string(),
        value: VCP_MOBILE_CLI_TOOL_NAME.to_string(),
    }];
    match action {
        VcpCliAction::Run {
            command,
            description,
            cwd,
            timeout_ms,
            run_in_background,
        } => {
            push_field(&mut fields, "action", "run");
            push_field(&mut fields, "command", command);
            if let Some(description) = description {
                push_field(&mut fields, "description", description);
            }
            if let Some(cwd) = cwd {
                push_field(&mut fields, "cwd", cwd);
            }
            if let Some(timeout_ms) = timeout_ms {
                push_field(&mut fields, "timeout_ms", timeout_ms.to_string());
            }
            if let Some(run_in_background) = run_in_background {
                push_field(
                    &mut fields,
                    "run_in_background",
                    run_in_background.to_string(),
                );
            }
        }
        VcpCliAction::ListSkills => push_field(&mut fields, "action", "list_skills"),
        VcpCliAction::ReadSkill {
            skill_id,
            resource_path,
            max_bytes,
        } => {
            push_field(&mut fields, "action", "read_skill");
            push_field(&mut fields, "skill_id", skill_id);
            if let Some(resource_path) = resource_path {
                push_field(&mut fields, "resource_path", resource_path);
            }
            if let Some(max_bytes) = max_bytes {
                push_field(&mut fields, "max_bytes", max_bytes.to_string());
            }
        }
        VcpCliAction::MaterializeSkill { skill_id } => {
            push_field(&mut fields, "action", "materialize_skill");
            push_field(&mut fields, "skill_id", skill_id);
        }
        VcpCliAction::Poll {
            job_id,
            cursor,
            max_output_bytes,
            wait_ms,
        } => {
            push_field(&mut fields, "action", "poll");
            push_field(&mut fields, "job_id", job_id);
            if let Some(cursor) = cursor {
                push_field(&mut fields, "cursor", cursor);
            }
            if let Some(max_output_bytes) = max_output_bytes {
                push_field(
                    &mut fields,
                    "max_output_bytes",
                    max_output_bytes.to_string(),
                );
            }
            if let Some(wait_ms) = wait_ms {
                push_field(&mut fields, "wait_ms", wait_ms.to_string());
            }
        }
        VcpCliAction::Cancel { job_id } => {
            push_field(&mut fields, "action", "cancel");
            push_field(&mut fields, "job_id", job_id);
        }
        VcpCliAction::List => push_field(&mut fields, "action", "list"),
    }
    validate_vcp_mobile_cli_request(&RawVcpToolRequest { fields }).map(|request| request.action)
}

fn push_field(fields: &mut Vec<RawVcpField>, key: &str, value: impl Into<String>) {
    fields.push(RawVcpField {
        key: key.to_string(),
        value: value.into(),
    });
}

impl VcpCliAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Run { .. } => "run",
            Self::ListSkills => "list_skills",
            Self::ReadSkill { .. } => "read_skill",
            Self::MaterializeSkill { .. } => "materialize_skill",
            Self::Poll { .. } => "poll",
            Self::Cancel { .. } => "cancel",
            Self::List => "list",
        }
    }
}

/// 唯一 validator 输出：只有类型化 action 到达 Runtime。上游 VCP 专属字段
/// （ink/archery/river/vref/签名等）不属于 mobile CLI 认知范围，在 validator 内静默丢弃。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidatedVcpCliRequest {
    #[serde(flatten)]
    pub action: VcpCliAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VcpCliProtocolError {
    pub code: VcpCliErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl VcpCliProtocolError {
    fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: VcpCliErrorCode::InvalidRequest,
            message: message.into(),
            field: Some(field.into()),
        }
    }
}

impl fmt::Display for VcpCliProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(field) = &self.field {
            write!(formatter, "{}: {}", field, self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for VcpCliProtocolError {}

/// 严格解析当前 VCP canonical marker。只返回完整闭合、至少含一个字段的 block。
/// `<think>/<thinking>`（含嵌套与未闭合块）会先按上游语义排除。
pub fn parse_vcp_tool_requests(content: &str) -> Vec<RawVcpToolRequest> {
    if content.is_empty() {
        return Vec::new();
    }

    let visible = strip_reasoning_blocks(content);
    let mut requests = Vec::new();
    let mut search_offset = 0;

    while search_offset < visible.len() {
        let Some(relative_start) = visible[search_offset..].find(TOOL_REQUEST_START) else {
            break;
        };
        let block_marker_start = search_offset + relative_start;
        let block_start = block_marker_start + TOOL_REQUEST_START.len();
        let Some((block_end, next_offset)) = find_block_end(&visible, block_start) else {
            // 与 VCPToolBox 一致：遇到未闭合 block 后保守停止，不扫描其内部的后续 marker。
            break;
        };

        let fields = scan_fields(visible[block_start..block_end].trim());
        if !fields.is_empty() {
            requests.push(RawVcpToolRequest { fields });
        }
        search_offset = next_offset;
    }

    requests
}

pub fn validate_vcp_mobile_cli_request(
    request: &RawVcpToolRequest,
) -> Result<ValidatedVcpCliRequest, VcpCliProtocolError> {
    // 先解析 action 以选择字段集：未知 action 仍然严厉拒绝；其余未知字段（如 maid/viad 等
    // VCP 专属签名字段）静默丢弃，而不是报错。这与 Distributed 入口的预过滤保持一致。
    let action_name = request
        .fields
        .iter()
        .find(|field| field.key == "action")
        .map(|field| field.value.trim())
        .unwrap_or("run");
    let allowed_fields = allowed_fields(action_name).ok_or_else(|| {
        VcpCliProtocolError::invalid(
            "action",
            "expected run|list_skills|read_skill|materialize_skill|poll|cancel|list",
        )
    })?;

    let mut values = HashMap::<&str, &str>::new();
    let mut duplicates = HashSet::<&str>::new();
    for field in &request.fields {
        if !allowed_fields.contains(&field.key.as_str()) {
            continue;
        }
        if values
            .insert(field.key.as_str(), field.value.trim())
            .is_some()
        {
            duplicates.insert(field.key.as_str());
        }
    }
    if let Some(field) = duplicates.into_iter().min() {
        return Err(VcpCliProtocolError::invalid(
            field,
            "duplicate fields are not allowed",
        ));
    }

    let tool_name = required_value(&values, "tool_name")?;
    if tool_name != VCP_MOBILE_CLI_TOOL_NAME {
        return Err(VcpCliProtocolError::invalid(
            "tool_name",
            format!("expected {VCP_MOBILE_CLI_TOOL_NAME}"),
        ));
    }

    let action = match action_name {
        "run" => validate_run(&values)?,
        "list_skills" => VcpCliAction::ListSkills,
        "read_skill" => validate_read_skill(&values)?,
        "materialize_skill" => VcpCliAction::MaterializeSkill {
            skill_id: validate_skill_id_field(&values)?,
        },
        "poll" => validate_poll(&values)?,
        "cancel" => VcpCliAction::Cancel {
            job_id: validate_identifier(required_value(&values, "job_id")?, "job_id")?,
        },
        "list" => VcpCliAction::List,
        _ => {
            return Err(VcpCliProtocolError::invalid(
                "action",
                "unreachable action variant",
            ));
        }
    };

    debug_assert_eq!(action.name(), action_name);
    Ok(ValidatedVcpCliRequest { action })
}

/// Convert Distributed `toolArgs` into the canonical raw-field representation and run the same
/// validator used by Human Tool markers. This deliberately does not deserialize `VcpCliAction`
/// directly: unknown fields are filtered by key here (before scalar coercion), so a foreign
/// signature field of any JSON shape is silently dropped instead of failing validation.
pub(crate) fn validate_distributed_vcp_cli_args(
    args: &Value,
) -> Result<ValidatedVcpCliRequest, VcpCliProtocolError> {
    let object = args
        .as_object()
        .ok_or_else(|| VcpCliProtocolError::invalid("toolArgs", "expected a JSON object"))?;

    // 先解析 action 选择字段集：未知 action 严厉拒绝；未知字段（含非标量 VCP 签名字段）
    // 在标量化之前静默丢弃，避免对象/数组形态的签名信息触发 "expected a scalar" 误拒。
    let action_name = match object.get("action") {
        None => "run".to_string(),
        Some(Value::String(value)) => value.trim().to_string(),
        Some(_) => {
            return Err(VcpCliProtocolError::invalid(
                "action",
                "expected run|list_skills|read_skill|materialize_skill|poll|cancel|list",
            ));
        }
    };
    let allowed = allowed_fields(&action_name).ok_or_else(|| {
        VcpCliProtocolError::invalid(
            "action",
            "expected run|list_skills|read_skill|materialize_skill|poll|cancel|list",
        )
    })?;

    let mut fields = Vec::with_capacity(object.len().saturating_add(1));
    if !object.contains_key("tool_name") {
        fields.push(RawVcpField {
            key: "tool_name".to_string(),
            value: VCP_MOBILE_CLI_TOOL_NAME.to_string(),
        });
    }
    for (key, value) in object {
        if !allowed.contains(&key.as_str()) {
            continue;
        }
        fields.push(RawVcpField {
            key: key.clone(),
            value: distributed_scalar_field(key, value)?,
        });
    }

    let validated = validate_vcp_mobile_cli_request(&RawVcpToolRequest { fields })?;
    Ok(validated)
}

fn distributed_scalar_field(key: &str, value: &Value) -> Result<String, VcpCliProtocolError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null => Err(VcpCliProtocolError::invalid(
            key,
            "null is not allowed; omit an optional field instead",
        )),
        Value::Array(_) | Value::Object(_) => Err(VcpCliProtocolError::invalid(
            key,
            "expected a scalar string, boolean, or number",
        )),
    }
}

fn strip_reasoning_blocks(content: &str) -> String {
    let mut chunks = Vec::new();
    let mut cursor = 0;
    let mut depth = 0_u32;

    for captures in REASONING_TAG.captures_iter(content) {
        let Some(whole_tag) = captures.get(0) else {
            continue;
        };
        if depth == 0 {
            chunks.push(&content[cursor..whole_tag.start()]);
        }

        let is_closing = captures
            .get(1)
            .is_some_and(|capture| capture.as_str() == "/");
        if is_closing {
            depth = depth.saturating_sub(1);
        } else {
            depth = depth.saturating_add(1);
        }
        cursor = whole_tag.end();
    }

    if depth == 0 {
        chunks.push(&content[cursor..]);
    }
    chunks.concat()
}

fn find_block_end(content: &str, from_index: usize) -> Option<(usize, usize)> {
    let mut cursor = from_index;

    while cursor < content.len() {
        let end_index = content[cursor..]
            .find(TOOL_REQUEST_END)
            .map(|relative| cursor + relative)?;
        let escape_start = ESCAPE_FIELD_START_ANYWHERE
            .find(&content[cursor..])
            .map(|matched| (cursor + matched.start(), cursor + matched.end()));

        if escape_start.is_none_or(|(start, _)| end_index < start) {
            return Some((end_index, end_index + TOOL_REQUEST_END.len()));
        }

        let (_, escape_content_start) = escape_start?;
        let escape_end = ESCAPE_FIELD_END.find(&content[escape_content_start..])?;
        cursor = escape_content_start + escape_end.end();
    }

    None
}

fn scan_fields(block: &str) -> Vec<RawVcpField> {
    let mut fields = Vec::new();
    let mut cursor = 0;

    while cursor < block.len() {
        cursor = skip_whitespace_and_commas(block, cursor);
        if cursor >= block.len() {
            break;
        }

        let key_start = cursor;
        while let Some(character) = block[cursor..].chars().next() {
            if !(character.is_ascii_alphanumeric() || character == '_') {
                break;
            }
            cursor += character.len_utf8();
        }
        if cursor == key_start {
            cursor += block[cursor..].chars().next().map_or(1, char::len_utf8);
            continue;
        }

        let key = &block[key_start..cursor];
        cursor = skip_whitespace(block, cursor);
        if !block[cursor..].starts_with(':') {
            continue;
        }
        cursor += ':'.len_utf8();
        cursor = skip_whitespace(block, cursor);

        let (value, next_cursor) = if let Some(start) = ESCAPE_FIELD_START.find(&block[cursor..]) {
            if start.start() != 0 {
                continue;
            }
            let content_start = cursor + start.end();
            let Some(end) = ESCAPE_FIELD_END.find(&block[content_start..]) else {
                break;
            };
            let content_end = content_start + end.start();
            (
                restore_escaped_literals(&block[content_start..content_end]),
                content_start + end.end(),
            )
        } else if block[cursor..].starts_with(FIELD_START) {
            let content_start = cursor + FIELD_START.len();
            let Some(relative_end) = block[content_start..].find(FIELD_END) else {
                break;
            };
            let content_end = content_start + relative_end;
            (
                block[content_start..content_end].to_string(),
                content_end + FIELD_END.len(),
            )
        } else {
            continue;
        };

        fields.push(RawVcpField {
            key: key.to_string(),
            value,
        });
        cursor = skip_whitespace(block, next_cursor);
        if block[cursor..].starts_with(',') {
            cursor += ','.len_utf8();
        }
    }

    fields
}

fn restore_escaped_literals(content: &str) -> String {
    let restored = content
        .replace(TOOL_REQUEST_START_ESCAPE, TOOL_REQUEST_START)
        .replace(TOOL_REQUEST_END_ESCAPE, TOOL_REQUEST_END);
    let restored = ESCAPE_FIELD_START_ANYWHERE.replace_all(&restored, FIELD_START);
    ESCAPE_FIELD_END
        .replace_all(&restored, FIELD_END)
        .into_owned()
}

fn skip_whitespace(content: &str, mut index: usize) -> usize {
    while let Some(character) = content[index..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        index += character.len_utf8();
    }
    index
}

fn skip_whitespace_and_commas(content: &str, mut index: usize) -> usize {
    while let Some(character) = content[index..].chars().next() {
        if !character.is_whitespace() && character != ',' {
            break;
        }
        index += character.len_utf8();
    }
    index
}

fn allowed_fields(action: &str) -> Option<&'static [&'static str]> {
    const RUN: &[&str] = &[
        "tool_name",
        "action",
        "command",
        "description",
        "cwd",
        "timeout_ms",
        "run_in_background",
    ];
    const LIST_SKILLS: &[&str] = &["tool_name", "action"];
    const READ_SKILL: &[&str] = &[
        "tool_name",
        "action",
        "skill_id",
        "resource_path",
        "max_bytes",
    ];
    const MATERIALIZE_SKILL: &[&str] = &["tool_name", "action", "skill_id"];
    const POLL: &[&str] = &[
        "tool_name",
        "action",
        "job_id",
        "cursor",
        "max_output_bytes",
        "wait_ms",
    ];
    const CANCEL: &[&str] = &["tool_name", "action", "job_id"];
    const LIST: &[&str] = &["tool_name", "action"];

    match action {
        "run" => Some(RUN),
        "list_skills" => Some(LIST_SKILLS),
        "read_skill" => Some(READ_SKILL),
        "materialize_skill" => Some(MATERIALIZE_SKILL),
        "poll" => Some(POLL),
        "cancel" => Some(CANCEL),
        "list" => Some(LIST),
        _ => None,
    }
}

fn parse_positive_u32(value: &str, field: &str) -> Result<u32, VcpCliProtocolError> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| VcpCliProtocolError::invalid(field, "expected a positive integer"))?;
    if parsed == 0 {
        return Err(VcpCliProtocolError::invalid(
            field,
            "expected a positive integer",
        ));
    }
    Ok(parsed)
}

fn validate_run(values: &HashMap<&str, &str>) -> Result<VcpCliAction, VcpCliProtocolError> {
    let command = required_value(values, "command")?;
    if command.len() > MAX_COMMAND_BYTES {
        return Err(VcpCliProtocolError::invalid(
            "command",
            format!("must not exceed {MAX_COMMAND_BYTES} UTF-8 bytes"),
        ));
    }
    reject_nul(command, "command")?;

    let description = optional_nonempty(values, "description")?;
    if let Some(description) = description {
        reject_nul(description, "description")?;
    }

    let cwd = values.get("cwd").copied().unwrap_or(DEFAULT_CWD);
    if cwd.is_empty() || !cwd.starts_with('/') {
        return Err(VcpCliProtocolError::invalid(
            "cwd",
            "must be an absolute guest path",
        ));
    }
    reject_nul(cwd, "cwd")?;

    let timeout_ms = parse_optional_u64(values, "timeout_ms", DEFAULT_TIMEOUT_MS)?;
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(VcpCliProtocolError::invalid(
            "timeout_ms",
            format!("must be in {MIN_TIMEOUT_MS}..={MAX_TIMEOUT_MS}"),
        ));
    }

    let run_in_background = match values.get("run_in_background") {
        None => false,
        Some(value) => parse_bool(value, "run_in_background")?,
    };

    Ok(VcpCliAction::Run {
        command: command.to_string(),
        description: description.map(str::to_string),
        cwd: Some(cwd.to_string()),
        timeout_ms: Some(timeout_ms),
        run_in_background: Some(run_in_background),
    })
}

fn validate_read_skill(values: &HashMap<&str, &str>) -> Result<VcpCliAction, VcpCliProtocolError> {
    let skill_id = validate_skill_id_field(values)?;

    let resource_path = values.get("resource_path").copied().unwrap_or("SKILL.md");
    validate_resource_path(resource_path)?;

    let max_bytes = parse_bounded_read_size(values, "max_bytes")?;
    Ok(VcpCliAction::ReadSkill {
        skill_id,
        resource_path: Some(resource_path.to_string()),
        max_bytes: Some(max_bytes),
    })
}

fn validate_skill_id_field(values: &HashMap<&str, &str>) -> Result<String, VcpCliProtocolError> {
    let skill_id = validate_identifier(required_value(values, "skill_id")?, "skill_id")?;
    if skill_id.contains(['/', '\\']) {
        return Err(VcpCliProtocolError::invalid(
            "skill_id",
            "must be a catalog identifier, not a path",
        ));
    }
    Ok(skill_id)
}

fn validate_poll(values: &HashMap<&str, &str>) -> Result<VcpCliAction, VcpCliProtocolError> {
    let job_id = validate_identifier(required_value(values, "job_id")?, "job_id")?;
    let cursor = optional_nonempty(values, "cursor")?.map(str::to_string);
    if cursor
        .as_ref()
        .is_some_and(|value| value.len() > MAX_CURSOR_BYTES)
    {
        return Err(VcpCliProtocolError::invalid(
            "cursor",
            format!("must not exceed {MAX_CURSOR_BYTES} UTF-8 bytes"),
        ));
    }

    let max_output_bytes = parse_bounded_read_size(values, "max_output_bytes")?;
    let wait_ms = parse_optional_u64(values, "wait_ms", 0)?;
    if wait_ms > MAX_POLL_WAIT_MS {
        return Err(VcpCliProtocolError::invalid(
            "wait_ms",
            format!("must be in 0..={MAX_POLL_WAIT_MS}"),
        ));
    }

    Ok(VcpCliAction::Poll {
        job_id,
        cursor,
        max_output_bytes: Some(max_output_bytes),
        wait_ms: Some(wait_ms),
    })
}

fn validate_resource_path(value: &str) -> Result<(), VcpCliProtocolError> {
    if value.is_empty() || value.len() > MAX_RESOURCE_PATH_BYTES || value.contains('\0') {
        return Err(VcpCliProtocolError::invalid(
            "resource_path",
            "must be a bounded relative file path",
        ));
    }

    let path = Path::new(value);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(VcpCliProtocolError::invalid(
            "resource_path",
            "must stay inside the selected Skill root",
        ));
    }
    Ok(())
}

fn parse_bounded_read_size(
    values: &HashMap<&str, &str>,
    field: &str,
) -> Result<usize, VcpCliProtocolError> {
    let requested = match values.get(field) {
        None => DEFAULT_BOUNDED_READ_BYTES,
        Some(value) => value.parse::<usize>().map_err(|_| {
            VcpCliProtocolError::invalid(field, "expected a positive integer byte count")
        })?,
    };
    if requested == 0 {
        return Err(VcpCliProtocolError::invalid(
            field,
            "expected a positive integer byte count",
        ));
    }
    Ok(requested.min(MAX_BOUNDED_READ_BYTES))
}

fn validate_identifier(value: &str, field: &str) -> Result<String, VcpCliProtocolError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.contains('\0') {
        return Err(VcpCliProtocolError::invalid(
            field,
            format!("must contain 1..={MAX_IDENTIFIER_BYTES} UTF-8 bytes"),
        ));
    }
    Ok(value.to_string())
}

fn required_value<'a>(
    values: &'a HashMap<&str, &str>,
    field: &str,
) -> Result<&'a str, VcpCliProtocolError> {
    values
        .get(field)
        .copied()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| VcpCliProtocolError::invalid(field, "field is required"))
}

fn optional_nonempty<'a>(
    values: &'a HashMap<&str, &str>,
    field: &str,
) -> Result<Option<&'a str>, VcpCliProtocolError> {
    match values.get(field).copied() {
        None => Ok(None),
        Some("") => Err(VcpCliProtocolError::invalid(
            field,
            "must not be empty when provided",
        )),
        Some(value) => Ok(Some(value)),
    }
}

fn parse_optional_u64(
    values: &HashMap<&str, &str>,
    field: &str,
    default: u64,
) -> Result<u64, VcpCliProtocolError> {
    values.get(field).map_or(Ok(default), |value| {
        value
            .parse::<u64>()
            .map_err(|_| VcpCliProtocolError::invalid(field, "expected an unsigned integer"))
    })
}

fn parse_bool(value: &str, field: &str) -> Result<bool, VcpCliProtocolError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(VcpCliProtocolError::invalid(field, "expected true|false")),
    }
}

fn reject_nul(value: &str, field: &str) -> Result<(), VcpCliProtocolError> {
    if value.contains('\0') {
        return Err(VcpCliProtocolError::invalid(field, "must not contain NUL"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct ParserGolden {
        cases: Vec<ParserCase>,
    }

    #[derive(Debug, Deserialize)]
    struct ParserCase {
        name: String,
        input: String,
        expected: Vec<Vec<RawFieldExpectation>>,
    }

    #[derive(Debug, Deserialize)]
    struct RawFieldExpectation {
        key: String,
        value: String,
    }

    #[derive(Debug, Deserialize)]
    struct ValidationGolden {
        valid: Vec<ValidCase>,
        invalid: Vec<InvalidCase>,
    }

    #[derive(Debug, Deserialize)]
    struct ValidCase {
        name: String,
        input: String,
        expected: Value,
    }

    #[derive(Debug, Deserialize)]
    struct InvalidCase {
        name: String,
        input: String,
        code: VcpCliErrorCode,
        field: String,
    }

    #[test]
    fn parser_matches_marker_escape_and_think_golden() {
        let fixture: ParserGolden =
            serde_json::from_str(include_str!("fixtures/vcp_tool_request_parser.golden.json"))
                .expect("parse parser golden fixture");

        for case in fixture.cases {
            let actual = parse_vcp_tool_requests(&case.input);
            let expected: Vec<Vec<(String, String)>> = case
                .expected
                .into_iter()
                .map(|request| {
                    request
                        .into_iter()
                        .map(|field| (field.key, field.value))
                        .collect()
                })
                .collect();
            let actual: Vec<Vec<(String, String)>> = actual
                .into_iter()
                .map(|request| {
                    request
                        .fields
                        .into_iter()
                        .map(|field| (field.key, field.value))
                        .collect()
                })
                .collect();
            assert_eq!(actual, expected, "parser case: {}", case.name);
        }
    }

    #[test]
    fn typed_validator_matches_action_and_error_golden() {
        let fixture: ValidationGolden =
            serde_json::from_str(include_str!("fixtures/vcp_cli_validation.golden.json"))
                .expect("parse validation golden fixture");

        for case in fixture.valid {
            let raw = exactly_one_request(&case.input, &case.name);
            let validated = validate_vcp_mobile_cli_request(&raw)
                .unwrap_or_else(|error| panic!("valid case {} rejected: {error}", case.name));
            let actual = serde_json::to_value(validated).expect("serialize validated request");
            assert_eq!(actual, case.expected, "validation case: {}", case.name);
        }

        for case in fixture.invalid {
            let raw = exactly_one_request(&case.input, &case.name);
            let error = validate_vcp_mobile_cli_request(&raw)
                .expect_err("invalid fixture must be rejected");
            assert_eq!(error.code, case.code, "invalid case: {}", case.name);
            assert_eq!(error.field.as_deref(), Some(case.field.as_str()));
        }
    }

    #[test]
    fn foreign_meta_and_signature_fields_are_silently_dropped() {
        let validated = validate_distributed_vcp_cli_args(&serde_json::json!({
            "command": "printf ok",
            "ink": "mark_history",
            "archery": "no_reply",
            "river": "text",
            "vref": 3,
            "maid": "Sakura",
            "unknown_meta": "x",
            "_vcpContext": {"agentId": "spoof"},
            "vref_files": ["file:///tmp/secret"],
            "river_context": {"content": "x"}
        }))
        .expect("upstream VCP fields must be dropped, not rejected");
        assert_eq!(
            validated.action,
            VcpCliAction::Run {
                command: "printf ok".to_string(),
                description: None,
                cwd: Some(DEFAULT_CWD.to_string()),
                timeout_ms: Some(DEFAULT_TIMEOUT_MS),
                run_in_background: Some(false),
            }
        );
        let encoded = serde_json::to_value(&validated).expect("serialize validated request");
        assert!(encoded.get("meta").is_none());
    }

    #[test]
    fn unknown_action_is_still_strictly_rejected() {
        let error = validate_distributed_vcp_cli_args(&serde_json::json!({
            "action": "execute",
            "command": "true"
        }))
        .expect_err("unknown action must stay fatal");
        assert_eq!(error.code, VcpCliErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("action"));

        let error = validate_distributed_vcp_cli_args(&serde_json::json!({
            "action": {"nested": true},
            "command": "true"
        }))
        .expect_err("non-scalar action must stay fatal");
        assert_eq!(error.code, VcpCliErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("action"));
    }

    #[test]
    fn distributed_json_args_reuse_canonical_action_validation() {
        let validated = validate_distributed_vcp_cli_args(&serde_json::json!({
            "action": "run",
            "command": "printf ok",
            "timeout_ms": 1200,
            "run_in_background": true
        }))
        .expect("valid Distributed args");

        assert_eq!(
            validated.action,
            VcpCliAction::Run {
                command: "printf ok".to_string(),
                description: None,
                cwd: Some(DEFAULT_CWD.to_string()),
                timeout_ms: Some(1200),
                run_in_background: Some(true),
            }
        );
    }

    #[test]
    fn known_field_invalid_values_still_fail_closed() {
        for (args, field) in [
            (serde_json::json!({"command": ["true"]}), "command"),
            (
                serde_json::json!({"command": "true", "description": null}),
                "description",
            ),
            (
                serde_json::json!({"tool_name": "OtherCLI", "command": "true"}),
                "tool_name",
            ),
            (
                serde_json::json!({"command": "true", "timeout_ms": 999}),
                "timeout_ms",
            ),
        ] {
            let error = validate_distributed_vcp_cli_args(&args)
                .expect_err("invalid values of known fields must fail closed");
            assert_eq!(error.code, VcpCliErrorCode::InvalidRequest);
            assert_eq!(error.field.as_deref(), Some(field));
        }
    }

    fn exactly_one_request(input: &str, name: &str) -> RawVcpToolRequest {
        let mut requests = parse_vcp_tool_requests(input);
        assert_eq!(requests.len(), 1, "fixture should contain one call: {name}");
        requests.remove(0)
    }
}
