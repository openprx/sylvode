-- Label colours are #rrggbb (stored lower-case) and names are unique per workspace ignoring case.
UPDATE labels SET color = '#6b7280' WHERE color !~ '^#[0-9a-fA-F]{6}$';
UPDATE labels SET color = lower(color) WHERE color <> lower(color);
UPDATE labels l SET name = l.name || ' (' || left(l.id::text, 8) || ')'
 WHERE EXISTS (SELECT 1 FROM labels o
                WHERE o.workspace_id = l.workspace_id AND lower(o.name) = lower(l.name)
                  AND (o.created_at, o.id) < (l.created_at, l.id));
ALTER TABLE labels ALTER COLUMN color SET DEFAULT '#6b7280';
ALTER TABLE labels DROP CONSTRAINT IF EXISTS labels_workspace_id_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS labels_workspace_lower_name_key ON labels (workspace_id, lower(name));
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'labels_color_hex') THEN
    ALTER TABLE labels ADD CONSTRAINT labels_color_hex CHECK (color ~ '^#[0-9a-f]{6}$');
  END IF;
END $$;
