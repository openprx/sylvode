-- Re-runnable.
-- Stored mentions of comments: a JSON array of {"id": <user id>, "label": <name when written>}.
-- The label is the name the mention was resolved from; readers show the current name and fall back
-- to the label for a person who has left. Work item comments and proposal comments get the column;
-- Flow comments already have one and keep their ids, now with labels.
ALTER TABLE comments
  ADD COLUMN IF NOT EXISTS mentions JSONB NOT NULL DEFAULT '[]'::jsonb
  CHECK (jsonb_typeof(mentions) = 'array' AND jsonb_array_length(mentions) <= 20);

ALTER TABLE proposal_comments
  ADD COLUMN IF NOT EXISTS mentions JSONB NOT NULL DEFAULT '[]'::jsonb
  CHECK (jsonb_typeof(mentions) = 'array' AND jsonb_array_length(mentions) <= 20);

-- Flow comments stored bare id strings. Rewrite them as {id, label}; an entry that is not an id is
-- dropped, as the reader always did.
UPDATE flow_comments c
SET mentions = COALESCE((
  SELECT jsonb_agg(
           jsonb_build_object('id', e.value #>> '{}', 'label', COALESCE(u.name, ''))
           ORDER BY e.ordinality)
  FROM jsonb_array_elements(c.mentions) WITH ORDINALITY AS e(value, ordinality)
  LEFT JOIN users u ON u.id::text = lower(e.value #>> '{}')
  WHERE jsonb_typeof(e.value) = 'string'
    AND (e.value #>> '{}') ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
), '[]'::jsonb)
WHERE EXISTS (
  SELECT 1 FROM jsonb_array_elements(c.mentions) AS x(value) WHERE jsonb_typeof(x.value) = 'string'
);
