-- Artefact drafts: specs authored by agents (form schemas, layouts) that wait for a human review.
-- Nothing in this table is live: approving a draft runs the ordinary save path with the approver's
-- rights and records the outcome in `result`. `review_digest` covers what the reviewer saw (spec,
-- validation, capability diff and public exposure); approve recomputes it and refuses a stale one.
--
-- Kinds `package_install` and `site_page` are admitted by the check so the later batches need no
-- constraint change; the API refuses them until their batch lands.
--
-- Safe to run again: every object is created only when absent, and the change hint trigger is
-- created only when its table does not carry it.

CREATE TABLE IF NOT EXISTS artefact_drafts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('form_schema','layout','package_install','site_page')),
  target JSONB NOT NULL,
  spec JSONB NOT NULL CHECK (octet_length(spec::text) <= 524288),
  author_kind TEXT NOT NULL CHECK (author_kind IN ('user','bot','oauth')),
  author_id UUID NOT NULL,
  note TEXT NULL CHECK (note IS NULL OR length(note) <= 2000),
  validation JSONB NOT NULL,
  capability_diff JSONB NOT NULL DEFAULT '[]'::jsonb,
  review_digest TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','rejected','superseded','failed')),
  reviewed_by UUID NULL REFERENCES users(id),
  reviewed_at TIMESTAMPTZ NULL,
  review_comment TEXT NULL CHECK (review_comment IS NULL OR length(review_comment) <= 2000),
  result JSONB NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(target) = 'object')
);
CREATE INDEX IF NOT EXISTS artefact_drafts_ws_status
  ON artefact_drafts (workspace_id, project_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS artefact_drafts_author
  ON artefact_drafts (project_id, author_kind, author_id, created_at DESC);

-- A draft was created or changed state: bots that can read the project re-read the list. Only
-- ids are sent. The block traps every error so a hint can never make a draft write fail.
CREATE OR REPLACE FUNCTION sv_hint_draft() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    PERFORM pg_notify('sv_change_hint', jsonb_build_object(
      'k', 'draft', 'id', NEW.id, 'p', NEW.project_id, 'w', NEW.workspace_id)::text);
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a draft (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

DO $$
BEGIN
  SET LOCAL lock_timeout = '10s';
  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('artefact_drafts') AND tgname = 'trg_sv_hint_draft_insert'
                   AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_sv_hint_draft_insert AFTER INSERT ON artefact_drafts
      FOR EACH ROW EXECUTE FUNCTION sv_hint_draft();
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('artefact_drafts') AND tgname = 'trg_sv_hint_draft_status'
                   AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_sv_hint_draft_status AFTER UPDATE OF status ON artefact_drafts
      FOR EACH ROW WHEN (OLD.status IS DISTINCT FROM NEW.status) EXECUTE FUNCTION sv_hint_draft();
  END IF;
END $$;
