-- Read-only public links for one Flow page. The raw token is never stored;
-- token_hash is SHA-256 hex of the full token. A link does not grant child objects.

CREATE TABLE IF NOT EXISTS flow_share_links (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_flow_share_links_object
    ON flow_share_links(object_id, created_at DESC);
