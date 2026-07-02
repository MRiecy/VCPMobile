-- Migration 0002: 为 message_attachments 表增加 deleted_at 逻辑删除字段
ALTER TABLE message_attachments ADD COLUMN deleted_at BIGINT;
