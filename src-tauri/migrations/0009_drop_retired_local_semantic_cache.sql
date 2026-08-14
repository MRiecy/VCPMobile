-- River/vref are compatibility-only in local loopback. Preserve migration 0008's
-- checksum for upgraded databases, then remove its retired device-local cache.
DROP INDEX IF EXISTS idx_local_semantic_cache_lru;
DROP TABLE IF EXISTS local_semantic_embedding_cache;
