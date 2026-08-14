use serde::{Deserialize, Serialize};

pub const VCP_MOBILE_CLI_TOOL_NAME: &str = "VCPMobileCLI";

const MANIFEST_DESCRIPTION: &str =
    "Android host 非 Root、PRoot 隔离的 Alpine Linux Bash CLI，支持异步 Job（仅前台可靠）与受控 Skill catalog。";

const INVOCATION_DESCRIPTION: &str = r#"在 Android 应用私有 PRoot guest 中执行非交互 Bash 命令，也可通过一等 action 发现、读取和显式物化 Mobile Skill。

环境：Shell 固定为 Alpine Linux(musl) 的 /bin/bash -lc；默认 cwd=/workspace；guest 模拟 root 仅用于隔离 rootfs 和 apk，不拥有 Android root；不支持 sudo/apt/systemctl/GUI/adb。基线包含 GNU 常用文件命令、grep/sed/awk/find/diff/patch、tar/zip、curl/wget、git/ssh、jq、python3/pip/apk；其他命令先用 command -v 检查。支持管道、重定向、&& 和多行 Bash。每次 action=run 启动独立 Bash Job。工作区文件和 apk 安装结果持久；cd/export/alias 不跨 run 保留。canonical Skill catalog 永不挂载进 Bash；list_skills/read_skill 只发现和阅读，materialize_skill 才把经 hash 校验的副本放入 /workspace/.vcp-skills。该副本可被 guest 修改但不会写回 catalog。协议兼容识别 river/vref，但不应用这两个可选上下文字段，Agent 不应主动发送或依赖它们；若旧请求携带，字段不会进入 Bash，命令继续执行并返回一条短提示。river/vref、远端 file:// 和伪造物化字段仍保持 fail-closed。

action: run(默认)|list_skills|read_skill|materialize_skill|poll|cancel|list。
- run 必需 command；可选 description、cwd、timeout_ms、run_in_background。普通 run 最多等待 8000ms，未完成也返回 job_id；run_in_background=true 立即返回 job_id。不要在 command 中用 nohup、setsid 或尾随 & 脱离任务，长任务使用 run_in_background。
- run 可附加 VCP 元字段 ink=mark_history、archery=true|no_reply；这些字段由工具层处理，不会拼入 Bash command。river/vref 仅作旧协议兼容，不是本地 CLI 能力。
- list_skills 返回已安装且校验通过的 Skill 索引。
- read_skill 必需 skill_id；resource_path 默认 SKILL.md，可选 max_bytes。返回有界正文与不可作为 shell 路径的逻辑 skill_root。
- materialize_skill 必需 skill_id；仅在没有活动 Job 时，把完整性校验通过的 Skill 复制为 /workspace 下的非回写快照并返回 materialized_path。它不会执行任何脚本；审阅后必须另发 run。
- poll/cancel 必需 job_id；poll 可选 cursor、max_output_bytes、wait_ms；list 返回当前 Runtime 保留的 Job 摘要。

run 是无 PTY、无交互 stdin 的调用；密码、vim/less/htop、交互 SSH 等当前不受 Agent Job 支持，必须改用非交互命令；独立人工终端属于 P5 能力。run_in_background 只表示工具异步 Job，不保证 Android 系统杀进程后继续。Job action 返回 state、job_id、stdout、stderr，以及终态 exit_code 或 timeout/cancel 原因；Skill action 返回结构化索引或正文、hash、逻辑 skill_root、可选 materialized_path 和截断状态。"#;

const INVOCATION_EXAMPLE: &str = r#"普通执行:
<<<[TOOL_REQUEST]>>>
tool_name:「始」VCPMobileCLI「末」,
action:「始」run「末」,
command:「始」find . -maxdepth 3 -type f | sort「末」,
description:「始」列出工作区文件「末」,
timeout_ms:「始」1800000「末」
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

异步执行（仅前台可靠）:
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
            timeout: 10_000,
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
        assert!(command
            .description
            .contains("ink=mark_history、archery=true|no_reply"));
        assert!(command.description.contains("river/vref 仅作旧协议兼容"));
        assert!(!command.description.contains("VCP_RIVER_CONTEXT_FILE"));
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
