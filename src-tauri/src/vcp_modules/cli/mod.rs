//! VCPMobileCLI 的 P0 协议切片。
//!
//! 本模块只拥有工具身份、Human Tool 请求解析/校验和结果投影合同；
//! Android 运行时、Distributed registry 与本地多轮 owner 均不在 P0 范围内。

#[allow(dead_code)]
mod ledger;
pub mod manifest;
#[allow(dead_code)]
mod output;
// P0 先冻结并校验资产合同，P1 runtime owner 接入后自然消费。
#[allow(dead_code)]
pub mod profile;
#[allow(dead_code)]
mod projection;
#[allow(dead_code)]
pub mod provision;
// P0 先冻结可复用协议/结果合同，P1 runtime 与 P2 local turn owner 接入后自然消费。
#[allow(dead_code)]
pub mod protocol;
#[allow(dead_code)]
pub mod result;
pub mod runtime;
#[allow(dead_code)]
mod skill_catalog;
#[allow(dead_code)]
mod skill_import;
#[allow(dead_code)]
mod skills;
#[allow(dead_code)]
pub(crate) mod turn_coordinator;
#[allow(dead_code)]
pub(crate) mod turn_ledger;
#[allow(dead_code)]
pub(crate) mod turn_meta;
#[allow(dead_code)]
pub(crate) mod turn_types;

pub use manifest::get_vcp_mobile_cli_manifest;
pub use runtime::{
    commit_vcp_mobile_cli_skill_import, discard_vcp_mobile_cli_skill_import,
    execute_vcp_mobile_cli_action, get_vcp_mobile_cli_skill_catalog, get_vcp_mobile_cli_status,
    inspect_vcp_mobile_cli_skill_import, MobileCliRuntimeState,
};
