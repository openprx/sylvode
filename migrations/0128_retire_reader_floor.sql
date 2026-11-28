-- Retires the Flow reader floor. A single-version deployment always runs at the floor the
-- shipped binaries register, so the lease table and the floor column carry no information:
-- the code behind every floor gate now runs unconditionally.
-- (flow_runtime_capability_rollouts was dropped by 0146.)
DROP TABLE IF EXISTS flow_runtime_readers;
ALTER TABLE flow_runtime_capabilities DROP COLUMN IF EXISTS minimum_reader_version;
