use serde::{Deserialize, Serialize};

pub const VCP_MOBILE_CLI_TOOL_NAME: &str = "VCPMobileCLI";

const MANIFEST_DESCRIPTION: &str =
    "手机端私有 Alpine Linux 沙箱中的非交互 Bash 执行器：后台 Job、游标分页输出与 Skill 资产，覆盖文件、构建、数据处理与网络类任务。";

const INVOCATION_DESCRIPTION: &str = r#"VCPMobileCLI — 在手机 Alpine 沙箱执行非交互 bash 命令，管理后台 Job 与 Skill。

action: run | list_skills | read_skill | materialize_skill | poll | cancel | list（未知 action 拒绝）

- run：command(必填) + description / cwd(默认 /workspace) / timeout_ms(默认30min,范围1s–12h) / run_in_background(默认false)
  长任务必须 run_in_background=true；前台最多等8s返回 job_id，state=running 时用 poll 跟踪
- poll：job_id(必填) + cursor / max_output_bytes(默认64KiB) / wait_ms(≤8s)
- cancel：job_id(必填)
- list：无参 → 当前 Job 摘要（找回 job_id）
- list_skills：无参
- read_skill：skill_id(必填) + resource_path(默认SKILL.md) / max_bytes
- materialize_skill：skill_id(必填)；流程 list_skills → read_skill → materialize_skill → run

输出：job.state / stdout / stderr / exit_code；exit_code≠0 或 state=failed 时先读 stderr

调用格式：
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」run「末」,
command:「始」...「末」
<<<[END_TOOL_REQUEST]>>>"#;

const INVOCATION_EXAMPLE: &str = r#"同步执行:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」run「末」,
command:「始」find . -maxdepth 3 -type f | sort「末」,
description:「始」列出工作区文件「末」
<<<[END_TOOL_REQUEST]>>>

列出 Skill:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」list_skills「末」
<<<[END_TOOL_REQUEST]>>>

读取 Skill:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」read_skill「末」,
skill_id:「始」example-skill「末」,
resource_path:「始」SKILL.md「末」
<<<[END_TOOL_REQUEST]>>>

物化 Skill 副本:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」materialize_skill「末」,
skill_id:「始」example-skill「末」
<<<[END_TOOL_REQUEST]>>>

后台执行:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」run「末」,
command:「始」python3 train.py「末」,
description:「始」运行本地训练任务「末」,
timeout_ms:「始」21600000「末」,
run_in_background:「始」true「末」
<<<[END_TOOL_REQUEST]>>>

查询任务:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」poll「末」,
job_id:「始」job_01J...「末」,
cursor:「始」c_42...「末」,
max_output_bytes:「始」65536「末」,
wait_ms:「始」8000「末」
<<<[END_TOOL_REQUEST]>>>

取消任务:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」cancel「末」,
job_id:「始」job_01J...「末」
<<<[END_TOOL_REQUEST]>>>"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpMobileCliManifest {
    pub manifest_version: String,
    pub name: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub author: String,
    pub plugin_type: String,
    pub entry_point: VcpMobileCliEntryPoint,
    pub communication: VcpMobileCliCommunication,
    pub capabilities: VcpMobileCliCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpMobileCliEntryPoint {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpMobileCliCommunication {
    pub protocol: String,
    pub timeout: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpMobileCliCapabilities {
    pub invocation_commands: Vec<VcpMobileCliInvocationCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpMobileCliInvocationCommand {
    pub command_identifier: String,
    pub description: String,
    pub example: String,
}

/// 唯一规范 manifest 构造器。本地导出与未来 Distributed adapter 必须调用这里。
pub fn vcp_mobile_cli_manifest() -> VcpMobileCliManifest {
    VcpMobileCliManifest {
        manifest_version: "1.0.0".to_string(),
        name: VCP_MOBILE_CLI_TOOL_NAME.to_string(),
        version: "1.0.0".to_string(),
        display_name: "VCP Mobile CLI".to_string(),
        description: MANIFEST_DESCRIPTION.to_string(),
        author: "VCPMobile".to_string(),
        plugin_type: "synchronous".to_string(),
        entry_point: VcpMobileCliEntryPoint {
            kind: "mobile".to_string(),
            command: "native".to_string(),
        },
        communication: VcpMobileCliCommunication {
            protocol: "mobile".to_string(),
            timeout: 30_000,
        },
        capabilities: VcpMobileCliCapabilities {
            invocation_commands: vec![VcpMobileCliInvocationCommand {
                command_identifier: VCP_MOBILE_CLI_TOOL_NAME.to_string(),
                description: INVOCATION_DESCRIPTION.to_string(),
                example: INVOCATION_EXAMPLE.to_string(),
            }],
        },
    }
}

pub fn serialize_vcp_mobile_cli_manifest() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&vcp_mobile_cli_manifest())
}

/// 前端 `invoke<string>('get_vcp_mobile_cli_manifest')` 得到 canonical JSON 文本。
/// 文本由唯一 typed manifest 构造器生成，使用两空格缩进且没有结尾换行；
/// 复制/导出侧不得二次 stringify。
#[tauri::command]
pub fn get_vcp_mobile_cli_manifest() -> Result<String, String> {
    serialize_vcp_mobile_cli_manifest().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_golden_snapshot_byte_for_byte() {
        let actual = serialize_vcp_mobile_cli_manifest().expect("serialize manifest fixture");
        let expected = include_str!("fixtures/vcp_mobile_cli_manifest.golden.json").trim_end();
        assert_eq!(actual, expected);
        assert_eq!(
            get_vcp_mobile_cli_manifest().expect("export canonical manifest"),
            actual
        );
        assert!(!actual.ends_with('\n'));
    }

    #[test]
    fn manifest_freezes_one_tool_identity_and_all_actions() {
        let manifest = vcp_mobile_cli_manifest();
        let command = &manifest.capabilities.invocation_commands[0];
        assert_eq!(manifest.name, VCP_MOBILE_CLI_TOOL_NAME);
        assert_eq!(command.command_identifier, VCP_MOBILE_CLI_TOOL_NAME);
        assert!(command.description.contains(
            "action: run | list_skills | read_skill | materialize_skill | poll | cancel | list"
        ));
        for banned in [
            "ink=", "archery", "river", "vref", "maid", "viad", "签名字段", "静默忽略", "不要发送",
        ] {
            assert!(
                !command.description.contains(banned),
                "manifest 不得提及上游 VCP 专属字段/机制: {banned}"
            );
        }
        assert!(!command.description.contains("VCP_RIVER_CONTEXT_FILE"));
        assert!(!command.description.contains("P5"));
        assert!(!command.description.contains("仅前台可靠"));
        for action in [
            "action:「始」run「末」",
            "action:「始」list_skills「末」",
            "action:「始」read_skill「末」",
            "action:「始」materialize_skill「末」",
            "action:「始」poll「末」",
            "action:「始」cancel「末」",
        ] {
            assert!(
                command.example.contains(action),
                "missing example: {action}"
            );
        }
    }
}
