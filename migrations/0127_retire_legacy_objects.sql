-- Retire objects that only served the legacy comment-anchor seal, the legacy `pages` import and the
-- retired plugin layout protocol. Forward-only: applied migrations stay as they are.
--
-- Every statement is safe to run again: the file may execute on a database whose ledger has no row
-- for it.

-- Comment anchors are bound when a comment is created; nothing is sealed afterwards.
ALTER TABLE flow_block_checkpoints DROP COLUMN IF EXISTS legacy_anchors_sealed;

-- The legacy `pages` table and its import: Flow package is the only import kind. A database that
-- still holds a legacy import row is refused with a clear message; no data is deleted.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM flow_import_jobs WHERE kind = 'legacy_pages') THEN
    RAISE EXCEPTION 'flow_import_jobs holds a legacy_pages import; resolve it before migration 0127';
  END IF;
  IF EXISTS (SELECT 1 FROM flow_import_lineage WHERE source_kind = 'legacy_pages') THEN
    RAISE EXCEPTION 'flow_import_lineage holds a legacy_pages row; resolve it before migration 0127';
  END IF;
END $$;

DROP TABLE IF EXISTS pages;

ALTER TABLE flow_import_jobs
  DROP CONSTRAINT IF EXISTS flow_import_jobs_kind_check,
  DROP CONSTRAINT IF EXISTS flow_import_jobs_legacy_pages_no_package_check,
  DROP CONSTRAINT IF EXISTS flow_import_jobs_legacy_pages_same_workspace_check,
  ADD CONSTRAINT flow_import_jobs_kind_check CHECK (kind = 'flow_package');

DROP INDEX IF EXISTS idx_flow_import_lineage_legacy_idempotency;

ALTER TABLE flow_import_lineage
  DROP CONSTRAINT IF EXISTS flow_import_lineage_source_kind_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_source_table_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_same_workspace_check,
  DROP CONSTRAINT IF EXISTS flow_import_lineage_target_shape_check,
  ADD CONSTRAINT flow_import_lineage_source_kind_check CHECK (source_kind = 'flow_package'),
  ADD CONSTRAINT flow_import_lineage_source_table_check CHECK (source_table IS NULL),
  ADD CONSTRAINT flow_import_lineage_target_shape_check CHECK (
    package_sha256 ~ '^[0-9a-f]{64}$'
    AND target_kind IN ('object', 'document', 'relation') AND target_id IS NOT NULL
    AND target_object_id IS NULL AND target_document_id IS NULL
  );

-- Plugin protocol 3 is the only plugin protocol; the retired layout table goes.
DROP TABLE IF EXISTS form_plugin_layouts;
