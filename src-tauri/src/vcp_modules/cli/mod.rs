//! VCPMobileCLI 的协议切片与 Android 运行时。
//!
//! 本模块拥有工具身份、Human Tool 请求解析/校验、结果投影合同与 PRoot 运行时；
//! 多轮 Agent loop 由 VCPToolBox 拥有，手机侧只承担单次 action 的幂等执行（runtime）。

#[allow(dead_code)]
mod ledger;
pub mod manifest;
#[allow(dead_code)]
mod output;
#[allow(dead_code)]
pub mod profile;
#[allow(dead_code)]
mod projection;
#[allow(dead_code)]
pub mod protocol;
#[allow(dead_code)]
pub mod provision;
#[allow(dead_code)]
pub mod result;
pub mod runtime;
#[allow(dead_code)]
mod skill_catalog;
#[allow(dead_code)]
mod skill_import;
#[allow(dead_code)]
mod skills;
pub mod terminal;

pub use manifest::get_vcp_mobile_cli_manifest;
pub use runtime::{
    commit_vcp_mobile_cli_skill_import, discard_vcp_mobile_cli_skill_import,
    execute_vcp_mobile_cli_action, get_vcp_mobile_cli_skill_catalog, get_vcp_mobile_cli_status,
    inspect_vcp_mobile_cli_skill_import, MobileCliRuntimeState,
};
pub use terminal::{
    close_vcp_mobile_cli_terminal, open_vcp_mobile_cli_terminal, read_vcp_mobile_cli_terminal,
    resize_vcp_mobile_cli_terminal, write_vcp_mobile_cli_terminal,
};
