use serde::{Deserialize, Serialize};

pub const VCP_MOBILE_CLI_TOOL_NAME: &str = "VCPMobileCLI";

const MANIFEST_DESCRIPTION: &str =
    "手机端私有 Alpine Linux 沙箱中的非交互 Bash 执行器：后台 Job、游标分页输出与 Skill 资产，覆盖文件、构建、数据处理与网络类任务。";

const INVOCATION_DESCRIPTION: &str = r#"在用户 Android 手机的应用私有 Alpine Linux 沙箱中执行非交互 Bash 命令，并管理后台 Job 与 Skill 资产。适合文件操作、数据处理、构建、脚本运行、网络请求等 shell 能完成的任务；不适合交互式程序（密码输入、vim/less、交互式 SSH）、系统管理（sudo/apt/systemctl）与 GUI/adb 操作。

环境（可直接依赖，不必先探测）：/bin/bash -lc；默认 cwd=/workspace，文件跨 run 持久，但 cd/export/alias 不保留；coreutils、grep/sed/awk/find/diff、tar/zip、curl/wget、git/ssh、jq、python3/pip 已预装，其他命令先用 command -v 确认；沙箱 root 仅用于隔离，不是 Android root。

action: run(默认)|list_skills|read_skill|materialize_skill|poll|cancel|list。
- run：command 必填，支持管道、重定向、&& 与多行脚本；可选 description（一句话目的，便于在 Job 列表辨认）、cwd、timeout_ms（默认 30 分钟，范围 1s–12h，超时任务被终止为 timed_out）、run_in_background（默认 false）。同步 run 最多等约 8 秒即返回（含 job_id），未完成时任务继续运行，用 poll 跟踪；run_in_background=true 立即返回 job_id。长任务必须用 run_in_background；禁止 nohup/setsid/尾随 &（脱离的任务会被清理）。
- poll：job_id 必填；可选 cursor（上次返回的游标，增量读取）、max_output_bytes（默认 64 KiB）、wait_ms（≤8 秒）。
- cancel：job_id 必填；终止 Job 的整个进程树。
- list：返回当前 Runtime 保留的 Job 摘要，用于找回 job_id。
- list_skills：返回已安装且校验通过的 Skill 索引。
- read_skill：skill_id 必填；resource_path 默认 SKILL.md，可选 max_bytes；返回有界正文与逻辑 skill_root（不是可用的 shell 路径）。
- materialize_skill：skill_id 必填，仅在没有活动 Job 时可用；把校验通过的 Skill 复制为 /workspace/.vcp-skills 下的快照并返回 materialized_path；它不执行任何脚本，审阅后用 run 执行。Skill 使用流程：list_skills → read_skill → materialize_skill → run。

结果解读：Job 返回 state（终态 completed|failed|timed_out|cancelled|interrupted，中间态 queued|starting|running）、stdout/stderr、exit_code 或超时/取消原因。exit_code≠0 或 state=failed 时，先读 stderr 定位原因，再决定重试或换命令；输出有界，截断时用 cursor 继续 poll。App 进程被系统杀死后后台 Job 不存活，重要结果尽快 poll 取回。

调用格式（每个 action 一条独立 TOOL_REQUEST 消息）：
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
            "action: run(默认)|list_skills|read_skill|materialize_skill|poll|cancel|list。"
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
