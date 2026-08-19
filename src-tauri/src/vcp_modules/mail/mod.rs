//! clawEmail 邮箱（VCPClawMail）：ClawEmail 云端邮箱的移动端查看与管理。
//!
//! 上游契约：`/admin_api/claw-mail/*`。
//! 详见 `plan/vcpmobile-more-tools-research/10-clawEmail-上游契约与移植方案.md`。

mod mail_service;

pub use mail_service::{
    mail_attachment, mail_folders, mail_list, mail_mark, mail_read, mail_reply, mail_search,
    mail_send, mail_state, mail_trash,
};
