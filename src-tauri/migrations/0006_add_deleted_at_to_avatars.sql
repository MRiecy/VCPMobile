-- Migration 0006: preserve avatar tombstones in the sync manifest.
ALTER TABLE avatars ADD COLUMN deleted_at BIGINT;
