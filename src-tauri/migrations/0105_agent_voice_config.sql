-- Agent voice identity and sample handles belong to this device only.
-- Sync upserts deliberately omit this column and retain its current value.
ALTER TABLE agents ADD COLUMN voice_config TEXT NOT NULL DEFAULT '{}'
    CHECK (json_valid(voice_config));
