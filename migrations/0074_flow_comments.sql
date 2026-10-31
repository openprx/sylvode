-- Flow comment threads. One level of replies, soft delete, explicit mention ids.
-- Object visibility stays on flow_objects grants; this table does not grant access.

CREATE TABLE IF NOT EXISTS flow_comments (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
    block_id TEXT,
    parent_id UUID REFERENCES flow_comments(id) ON DELETE CASCADE,
    author_id UUID REFERENCES users(id) ON DELETE SET NULL,
    body TEXT NOT NULL,
    mentions JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    resolved_by UUID REFERENCES users(id) ON DELETE SET NULL,
    deleted_at TIMESTAMPTZ,
    CONSTRAINT flow_comments_body_check CHECK (length(trim(body)) BETWEEN 1 AND 8000),
    CONSTRAINT flow_comments_block_len CHECK (block_id IS NULL OR length(block_id) BETWEEN 1 AND 128),
    CONSTRAINT flow_comments_not_self CHECK (parent_id IS NULL OR parent_id <> id),
    CONSTRAINT flow_comments_mentions_array CHECK (jsonb_typeof(mentions) = 'array'),
    CONSTRAINT flow_comments_mentions_cap CHECK (jsonb_array_length(mentions) <= 20)
);

CREATE INDEX IF NOT EXISTS idx_flow_comments_object
    ON flow_comments(object_id, created_at)
    WHERE deleted_at IS NULL;
