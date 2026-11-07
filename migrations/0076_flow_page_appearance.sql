-- Page icon and cover. The collaborative document stays the source of block text;
-- appearance is governance metadata so the navigator can show an icon without opening the doc.

ALTER TABLE flow_objects
  ADD COLUMN IF NOT EXISTS appearance JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE flow_objects
  DROP CONSTRAINT IF EXISTS flow_objects_appearance_object_check;

ALTER TABLE flow_objects
  ADD CONSTRAINT flow_objects_appearance_object_check
    CHECK (jsonb_typeof(appearance) = 'object');

COMMENT ON COLUMN flow_objects.appearance IS
  'Page icon (emoji or uploaded image) and cover (gallery gradient or upload, with a 0..1 vertical position)';

CREATE TABLE IF NOT EXISTS flow_appearance_assets (
  id UUID PRIMARY KEY,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  bytes BYTEA NOT NULL,
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_appearance_assets_type_check
    CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif')),
  CONSTRAINT flow_appearance_assets_size_check
    CHECK (byte_size > 0 AND byte_size <= 2097152 AND byte_size = octet_length(bytes))
);

CREATE INDEX IF NOT EXISTS idx_flow_appearance_assets_object
  ON flow_appearance_assets(object_id, id);
