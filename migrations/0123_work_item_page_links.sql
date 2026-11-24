-- Re-runnable.
-- Links between work items and Flow pages. A link is a plain pair; the page title is read live
-- when a link is shown, so a rename never leaves a stale copy here. Deleting the work item or
-- purging the page removes its links; moving a page to the trash keeps them.
CREATE TABLE IF NOT EXISTS work_item_page_links (
  work_item_id UUID NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  object_id    UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  created_by   UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (work_item_id, object_id)
);

CREATE INDEX IF NOT EXISTS work_item_page_links_object ON work_item_page_links (object_id);
