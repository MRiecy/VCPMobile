//! 日志中心（VCP Log Center）：远程查看 VCPToolBox 服务器日志。
//!
//! 上游契约：`GET /admin_api/server-log`（全量尾部 2MB / 增量字节 offset /
//! inode 轮转检测）+ `POST /admin_api/server-log/clear`。
//! 详见 `plan/vcpmobile-more-tools-research/01-日志中心-上游契约与移植方案.md`。

mod log_service;

pub use log_service::{logcenter_clear_server, logcenter_fetch};
