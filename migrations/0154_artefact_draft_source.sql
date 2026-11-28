-- Provenance of an artefact draft written by a plugin call: the installation, its generation and
-- the package key, kept next to the invoking person as author. `NULL` for a draft a person or a
-- bot wrote directly. A plugin's draft supersedes only pending drafts of the same installation, so
-- it never replaces the person's own pending draft for the same target.
-- Safe to run again.

ALTER TABLE artefact_drafts
  ADD COLUMN IF NOT EXISTS source JSONB NULL;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'artefact_drafts_source_shape') THEN
    ALTER TABLE artefact_drafts ADD CONSTRAINT artefact_drafts_source_shape
      CHECK (source IS NULL OR (jsonb_typeof(source) = 'object' AND source ? 'installation_id'));
  END IF;
END $$;
