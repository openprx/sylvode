-- One active version per plugin key in a project, and the restaurant template no longer lists the
-- constant-output calculation plugin (its line total is a formula field now). Re-runnable.
UPDATE scenario_templates
   SET ai_roles = (SELECT COALESCE(jsonb_agg(r), '[]'::jsonb)
                     FROM jsonb_array_elements(ai_roles) r
                    WHERE r->>'key' <> 'restaurant_calc')
 WHERE key = 'restaurant_ordering_default' AND workspace_id IS NULL
   AND ai_roles @> '[{"key":"restaurant_calc"}]';
UPDATE plugins p SET status = 'disabled', updated_at = now()
 WHERE p.status = 'active'
   AND EXISTS (SELECT 1 FROM plugins o
                WHERE o.project_id = p.project_id AND o.key = p.key AND o.status = 'active'
                  AND (o.updated_at, o.id) > (p.updated_at, p.id));
CREATE UNIQUE INDEX IF NOT EXISTS plugins_one_active_version
  ON plugins (project_id, key) WHERE status = 'active';
