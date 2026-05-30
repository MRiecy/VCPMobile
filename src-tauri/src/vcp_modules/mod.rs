// 1. 声明 7 大物理子领域
pub mod agent;
pub mod chat;
pub mod group;
pub mod infra;
pub mod persistence;
pub mod sync;
pub mod updater;

// 2. 扁平化外观代理导出 (Façade Re-exporting)
// 完美兼容 lib.rs 和外部文件对原有扁平模块的引用

// --- Agent 领域 ---
pub use agent::agent_chat_application_service;
pub use agent::agent_service;
pub use agent::agent_types;
pub use agent::avatar_service;

// --- Group 领域 ---
pub use group::group_chat_application_service;
pub use group::group_context_assembler;
pub use group::group_service;
pub use group::group_speaking_policy;
pub use group::group_types;

// --- Chat 领域 ---
pub use chat::aurora_pipeline;
pub use chat::chat_manager;
pub use chat::content_parser;
pub use chat::context_assembler;
pub use chat::context_injection;
pub use chat::context_sanitizer;
pub use chat::emoticon_manager;
pub use chat::message_service;
pub use chat::pre_renderer;
pub use chat::stream_block_parser;
pub use chat::topic_service;
pub use chat::topic_summary_service;
pub use chat::topic_types;

// --- Sync 领域 ---
pub use sync::sync_dto;
pub use sync::sync_executor;
pub use sync::sync_hash;
pub use sync::sync_logger;
pub use sync::sync_pipeline;
pub use sync::sync_service;
pub use sync::sync_types;

// --- Persistence 领域 ---
pub use persistence::db_manager;
pub use persistence::db_write_queue;
pub use persistence::message_repository;

// --- Infra 领域 ---
pub use infra::file_manager;
pub use infra::high_speed_channel;
pub use infra::lifecycle_manager;
pub use infra::maintenance_manager;
pub use infra::media_processor;
pub use infra::model_manager;
pub use infra::settings_manager;
pub use infra::vcp_client;
pub use infra::vcp_log_service;

// --- Updater 领域 ---
pub use updater::frontend_update_manager;
pub use updater::ota_assets;
pub use updater::update_manager;
