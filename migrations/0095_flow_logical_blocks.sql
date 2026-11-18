-- Phase3 candidate only: registration/deployment follows complete accepted-write integration.
-- Logical identities are server-owned, never read from client CRDT attributes.
CREATE TABLE IF NOT EXISTS flow_logical_blocks (
 id UUID PRIMARY KEY,
 state TEXT NOT NULL CHECK(state IN ('active','deleted','purged')),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS flow_block_placements (
 id UUID PRIMARY KEY,
 alias_order BIGSERIAL NOT NULL UNIQUE,
 logical_id UUID NOT NULL REFERENCES flow_logical_blocks(id),
 scope_id UUID NOT NULL,
 page_id UUID NOT NULL,
 physical_cid TEXT NOT NULL CHECK(octet_length(physical_cid) BETWEEN 1 AND 256),
 generation BIGINT NOT NULL CHECK(generation>0),
 state TEXT NOT NULL CHECK(state IN ('active','deleted','moved')),
 content_hash BYTEA NOT NULL CHECK(octet_length(content_hash)=32),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 retired_at TIMESTAMPTZ,
 FOREIGN KEY(scope_id,page_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE,
 UNIQUE(logical_id,generation),
 CHECK((state='active')=(retired_at IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS flow_block_one_active_logical ON flow_block_placements(logical_id) WHERE state='active';
CREATE UNIQUE INDEX IF NOT EXISTS flow_block_one_active_physical ON flow_block_placements(scope_id,page_id,physical_cid) WHERE state='active';
CREATE INDEX IF NOT EXISTS flow_block_alias_history ON flow_block_placements(scope_id,page_id,physical_cid,alias_order DESC);
-- Checkpoint is private source state. Never returned as recipient revision.
CREATE TABLE IF NOT EXISTS flow_block_checkpoints (
 scope_id UUID NOT NULL,
 page_id UUID NOT NULL,
 accepted_head BIGINT NOT NULL CHECK(accepted_head>=0),
 legacy_anchors_sealed BOOLEAN NOT NULL DEFAULT false,
 PRIMARY KEY(scope_id,page_id),
 FOREIGN KEY(scope_id,page_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE
);
-- No audience/grant defaults: registry existence does not authorize any recipient.

-- Source ownership stays private; destination audiences are not source Page grants.
CREATE TABLE IF NOT EXISTS flow_block_shares (
 id UUID PRIMARY KEY,
 source_scope_id UUID NOT NULL,
 source_page_id UUID NOT NULL,
 source_root_cid TEXT NOT NULL CHECK(octet_length(source_root_cid) BETWEEN 1 AND 256),
 root_logical_id UUID NOT NULL REFERENCES flow_logical_blocks(id),
 selection TEXT NOT NULL CHECK(selection IN ('block','current_subtree')),
 created_by UUID NOT NULL REFERENCES users(id),
 publisher_user_id UUID NOT NULL REFERENCES users(id),
 state TEXT NOT NULL CHECK(state IN ('active','suspended','revoked','target_removed')),
 content_revision BIGINT NOT NULL DEFAULT 1 CHECK(content_revision>0),
 projection_hash BYTEA NOT NULL DEFAULT decode(repeat('00',32),'hex') CHECK(octet_length(projection_hash)=32),
 auth_revision BIGINT NOT NULL DEFAULT 1 CHECK(auth_revision>0),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(id,source_scope_id,source_page_id),
 FOREIGN KEY(source_scope_id,source_page_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS flow_block_share_members (
 share_id UUID NOT NULL REFERENCES flow_block_shares(id) ON DELETE CASCADE,
 member_id UUID NOT NULL,
 logical_id UUID NOT NULL REFERENCES flow_logical_blocks(id),
 parent_member_id UUID,
 member_order INTEGER NOT NULL CHECK(member_order>=0),
 content_hash BYTEA NOT NULL CHECK(octet_length(content_hash)=32),
 PRIMARY KEY(share_id,member_id),
 UNIQUE(share_id,logical_id),
 FOREIGN KEY(share_id,parent_member_id) REFERENCES flow_block_share_members(share_id,member_id) DEFERRABLE INITIALLY DEFERRED,
 CHECK(parent_member_id IS NULL OR parent_member_id<>member_id)
);
CREATE INDEX IF NOT EXISTS flow_block_member_logical ON flow_block_share_members(logical_id,share_id);
CREATE TABLE IF NOT EXISTS flow_block_share_placements (
 id UUID PRIMARY KEY,
 share_id UUID NOT NULL UNIQUE REFERENCES flow_block_shares(id) ON DELETE CASCADE,
 workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
 project_key UUID GENERATED ALWAYS AS (coalesce(project_id,'00000000-0000-0000-0000-000000000000'::uuid)) STORED,
 state TEXT NOT NULL CHECK(state IN ('active','removed')),
 UNIQUE(id,share_id,workspace_id,project_key)
);
CREATE UNIQUE INDEX IF NOT EXISTS flow_block_placement_target ON flow_block_share_placements(share_id,workspace_id,project_key);
CREATE TABLE IF NOT EXISTS flow_block_share_audiences (
 id UUID PRIMARY KEY,
 placement_id UUID NOT NULL,
 share_id UUID NOT NULL REFERENCES flow_block_shares(id) ON DELETE CASCADE,
 workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
 audience_kind TEXT NOT NULL CHECK(audience_kind IN ('workspace','user')),
 recipient_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
 permission TEXT NOT NULL CHECK(permission IN ('view','edit')),
 state TEXT NOT NULL CHECK(state IN ('active','removed')),
 CHECK((audience_kind='workspace')=(recipient_user_id IS NULL)),
 project_key UUID GENERATED ALWAYS AS (coalesce(project_id,'00000000-0000-0000-0000-000000000000'::uuid)) STORED,
 FOREIGN KEY(placement_id,share_id,workspace_id,project_key) REFERENCES flow_block_share_placements(id,share_id,workspace_id,project_key) ON DELETE CASCADE,
 -- A project is a display location, never an implicit grant to all workspace members.
 CHECK(project_id IS NULL OR audience_kind='user')
);
CREATE TABLE IF NOT EXISTS flow_block_share_asset_aliases (
 share_id UUID NOT NULL,
 member_id UUID NOT NULL,
 alias_id UUID NOT NULL UNIQUE,
 source_scope_id UUID NOT NULL,
 source_page_id UUID NOT NULL,
 asset_id UUID NOT NULL,
 PRIMARY KEY(share_id,member_id,asset_id),
 FOREIGN KEY(share_id,member_id) REFERENCES flow_block_share_members(share_id,member_id) ON DELETE CASCADE,
 FOREIGN KEY(share_id,source_scope_id,source_page_id) REFERENCES flow_block_shares(id,source_scope_id,source_page_id) ON DELETE CASCADE,
 FOREIGN KEY(source_scope_id,source_page_id,asset_id) REFERENCES flow_page_assets(scope_id,object_id,id)
);
-- Acknowledgements contain no historical projection/source sequence. Replay reauthorizes current view.
CREATE TABLE IF NOT EXISTS flow_block_share_receipts (
 share_id UUID NOT NULL REFERENCES flow_block_shares(id) ON DELETE CASCADE,
 actor_id UUID NOT NULL REFERENCES users(id),
 idempotency_key TEXT NOT NULL CHECK(octet_length(idempotency_key) BETWEEN 1 AND 128),
 request_body JSONB NOT NULL,
 accepted_revision BIGINT NOT NULL CHECK(accepted_revision>0),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(share_id,actor_id,idempotency_key)
);

-- Lifecycle mutations do not necessarily advance Page content. Suspend durably in that transaction.
CREATE OR REPLACE FUNCTION flow_block_source_object_lifecycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE source_page_id=OLD.id AND state='active';
  UPDATE flow_logical_blocks SET state='purged' WHERE id IN (SELECT logical_id FROM flow_block_placements WHERE page_id=OLD.id AND state='active');
  RETURN OLD;
 END IF;
 IF NEW.lifecycle_status<>'active' THEN
  UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE source_page_id=NEW.id AND state='active';
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_block_source_object_archive ON flow_objects;
CREATE TRIGGER flow_block_source_object_archive AFTER UPDATE OF lifecycle_status ON flow_objects
 FOR EACH ROW WHEN(OLD.lifecycle_status IS DISTINCT FROM NEW.lifecycle_status)
 EXECUTE FUNCTION flow_block_source_object_lifecycle();
DROP TRIGGER IF EXISTS flow_block_source_object_delete ON flow_objects;
CREATE TRIGGER flow_block_source_object_delete BEFORE DELETE ON flow_objects
 FOR EACH ROW EXECUTE FUNCTION flow_block_source_object_lifecycle();
CREATE OR REPLACE FUNCTION flow_block_source_scope_lifecycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.lifecycle_status<>'active' THEN
  UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE source_scope_id=NEW.id AND state='active';
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_block_source_scope_archive ON flow_scopes;
CREATE TRIGGER flow_block_source_scope_archive AFTER UPDATE OF lifecycle_status ON flow_scopes
 FOR EACH ROW WHEN(OLD.lifecycle_status IS DISTINCT FROM NEW.lifecycle_status)
 EXECUTE FUNCTION flow_block_source_scope_lifecycle();
CREATE OR REPLACE FUNCTION flow_block_target_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='projects' THEN
  UPDATE flow_block_shares SET state='target_removed',auth_revision=auth_revision+1 WHERE state<>'revoked' AND id IN (SELECT share_id FROM flow_block_share_placements WHERE project_id=OLD.id);
 ELSE
  UPDATE flow_block_shares SET state='target_removed',auth_revision=auth_revision+1 WHERE state<>'revoked' AND id IN (SELECT share_id FROM flow_block_share_placements WHERE workspace_id=OLD.id);
 END IF;
 RETURN OLD;
END $$;
-- CASCADE removes only target bindings/grants. No SET NULL that could broaden project authority.
DROP TRIGGER IF EXISTS flow_block_target_project_delete ON projects;
CREATE TRIGGER flow_block_target_project_delete BEFORE DELETE ON projects FOR EACH ROW EXECUTE FUNCTION flow_block_target_delete();
DROP TRIGGER IF EXISTS flow_block_target_workspace_delete ON workspaces;
CREATE TRIGGER flow_block_target_workspace_delete BEFORE DELETE ON workspaces FOR EACH ROW EXECUTE FUNCTION flow_block_target_delete();

CREATE TABLE IF NOT EXISTS flow_block_comment_anchors (
 comment_id UUID PRIMARY KEY REFERENCES flow_comments(id) ON DELETE CASCADE,
 source_page_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
 original_block_id TEXT NOT NULL,
 logical_id UUID REFERENCES flow_logical_blocks(id),
 origin_placement_id UUID REFERENCES flow_block_placements(id) ON DELETE SET NULL,
 origin_generation BIGINT CHECK(origin_generation>0),
 state TEXT NOT NULL CHECK(state IN ('known','orphaned')),
 CHECK((state='known')=(logical_id IS NOT NULL AND origin_generation IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS flow_block_comment_logical ON flow_block_comment_anchors(logical_id,source_page_id);

-- Minimal retirement evidence survives optional pruning of placement history rows.
CREATE TABLE IF NOT EXISTS flow_block_alias_fences (
 scope_id UUID NOT NULL,
 page_id UUID NOT NULL,
 physical_cid TEXT NOT NULL,
 first_logical_id UUID NOT NULL REFERENCES flow_logical_blocks(id),
 ever_retired BOOLEAN NOT NULL DEFAULT false,
 PRIMARY KEY(scope_id,page_id,physical_cid),
 FOREIGN KEY(scope_id,page_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE
);
CREATE OR REPLACE FUNCTION flow_block_retire_alias() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  UPDATE flow_block_alias_fences SET ever_retired=true WHERE scope_id=OLD.scope_id AND page_id=OLD.page_id AND physical_cid=OLD.physical_cid;
  RETURN OLD;
 END IF;
 IF NEW.state<>'active' THEN
  INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired)
  VALUES(NEW.scope_id,NEW.page_id,NEW.physical_cid,NEW.logical_id,true)
  ON CONFLICT(scope_id,page_id,physical_cid) DO UPDATE SET ever_retired=true;
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_block_alias_retire ON flow_block_placements;
CREATE TRIGGER flow_block_alias_retire AFTER UPDATE OF state ON flow_block_placements
 FOR EACH ROW EXECUTE FUNCTION flow_block_retire_alias();
DROP TRIGGER IF EXISTS flow_block_alias_prune ON flow_block_placements;
CREATE TRIGGER flow_block_alias_prune BEFORE DELETE ON flow_block_placements
 FOR EACH ROW EXECUTE FUNCTION flow_block_retire_alias();
