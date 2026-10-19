-- 群组本机行为不进入同步 DTO/hash。无外键以支持先导入备份、后同步群身份。
CREATE TABLE IF NOT EXISTS group_local_settings (
    group_id TEXT PRIMARY KEY NOT NULL,
    settings_json TEXT NOT NULL CHECK (json_valid(settings_json)),
    updated_at BIGINT NOT NULL
);

-- 本地删除和远端墓碑沿同一 groups owner 生效；普通同步更新不触碰本机设置。
CREATE TRIGGER IF NOT EXISTS cleanup_group_local_settings_tombstone
AFTER UPDATE OF deleted_at ON groups
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM group_local_settings WHERE group_id = NEW.group_id;
END;

CREATE TRIGGER IF NOT EXISTS cleanup_group_local_settings_deleted
AFTER DELETE ON groups
BEGIN
    DELETE FROM group_local_settings WHERE group_id = OLD.group_id;
END;

CREATE TRIGGER IF NOT EXISTS cleanup_group_local_settings_new_tombstone
AFTER INSERT ON groups
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM group_local_settings WHERE group_id = NEW.group_id;
END;
