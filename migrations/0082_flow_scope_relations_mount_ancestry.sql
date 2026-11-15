-- A mount authorizes its source subtree only through the same first-boundary ancestry contract
-- as REST/WS. Validate the complete canonical tree before selecting an authorization prefix.
CREATE OR REPLACE FUNCTION flow_mount_acl_ancestors(requested_object UUID,requested_scope UUID)
RETURNS TABLE(object_id UUID) LANGUAGE sql STABLE AS $$
 WITH RECURSIVE chain AS(
  SELECT o.id,o.parent_id,o.inherit_from_parent,0 AS depth,ARRAY[o.id]::uuid[] AS path,false AS cycle,
   flow_is_system_navigator_root(o.object_type,o.parent_id,o.governance_metadata) AS hidden_root
   FROM flow_objects o WHERE o.id=requested_object AND o.scope_id=requested_scope
  UNION ALL
  SELECT p.id,p.parent_id,p.inherit_from_parent,c.depth+1,c.path||p.id,p.id=ANY(c.path),
   flow_is_system_navigator_root(p.object_type,p.parent_id,p.governance_metadata)
   FROM chain c JOIN flow_objects p ON p.id=c.parent_id AND p.scope_id=requested_scope
   WHERE c.parent_id IS NOT NULL AND c.depth<34 AND NOT c.cycle
 ), valid AS(
  SELECT NOT COALESCE(bool_or(cycle),false)
   AND COALESCE(bool_or(parent_id IS NULL),false)
   AND count(*)<=33+CASE WHEN COALESCE(bool_or(hidden_root),false) THEN 1 ELSE 0 END AS okay,
   min(depth) FILTER(WHERE NOT inherit_from_parent) AS boundary FROM chain
 ) SELECT c.id FROM chain c CROSS JOIN valid v WHERE v.okay AND (v.boundary IS NULL OR c.depth<=v.boundary)
$$;
DO $migration$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='flow_relations' AND column_name='scope_id') THEN
 EXECUTE $ddl$
ALTER TABLE flow_relations ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_relations ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_relations r SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=r.source_object_id;
CREATE TRIGGER flow_relations_bridge_scope BEFORE INSERT OR UPDATE ON flow_relations FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_relations ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_relations ADD CONSTRAINT flow_relations_source_scope_fk FOREIGN KEY(scope_id,source_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_relations ADD CONSTRAINT flow_relations_target_scope_fk FOREIGN KEY(scope_id,target_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
CREATE UNIQUE INDEX flow_relations_scope_link_key ON flow_relations(scope_id,source_object_id,target_object_id,relation_type);
$ddl$;
 END IF;
END $migration$;
