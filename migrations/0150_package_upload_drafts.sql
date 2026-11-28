-- Package upload drafts (L4.A3): a bot never creates a package row itself. It submits a signed
-- `.svpkg`; the server verifies it against the workspace's registered publisher keys, keeps the
-- bytes in `package_upload_blobs` next to a `package_upload` draft and waits for a workspace owner
-- or administrator to approve it. Approval runs the ordinary package store path.
--
-- A package upload belongs to a workspace, not to a project: `artefact_drafts.project_id` becomes
-- nullable, and only this kind may leave it empty. The blob row is deleted when the draft leaves
-- `pending` (approved, rejected or superseded) and with the draft itself.
--
-- Re-runnable: every statement tolerates the new state.

ALTER TABLE artefact_drafts ALTER COLUMN project_id DROP NOT NULL;

ALTER TABLE artefact_drafts DROP CONSTRAINT IF EXISTS artefact_drafts_kind_check;
ALTER TABLE artefact_drafts ADD CONSTRAINT artefact_drafts_kind_check
  CHECK (kind IN ('form_schema','layout','package_install','site_page','package_upload'));

ALTER TABLE artefact_drafts DROP CONSTRAINT IF EXISTS artefact_drafts_project_by_kind;
ALTER TABLE artefact_drafts ADD CONSTRAINT artefact_drafts_project_by_kind
  CHECK ((project_id IS NULL) = (kind = 'package_upload'));

CREATE TABLE IF NOT EXISTS package_upload_blobs (
  draft_id UUID PRIMARY KEY REFERENCES artefact_drafts(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  blob_digest TEXT NOT NULL,
  size BIGINT NOT NULL CHECK (size > 0 AND size <= 33554432),
  bytes BYTEA NOT NULL CHECK (octet_length(bytes) = size)
);
CREATE INDEX IF NOT EXISTS package_upload_blobs_workspace ON package_upload_blobs (workspace_id);
