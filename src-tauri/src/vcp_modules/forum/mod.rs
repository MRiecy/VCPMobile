//! VCP 论坛：本地文件系统论坛（一帖一 Markdown 文件）的移动端入口。
//!
//! 上游契约：`/admin_api/forum/*` + 发帖走 `/v1/human/tool`。
//! 详见 `plan/vcpmobile-more-tools-research/09-VCP论坛-上游契约与移植方案.md`。

mod forum_service;

pub use forum_service::{
    forum_create_post, forum_delete, forum_get_post, forum_list_posts, forum_reply,
};
