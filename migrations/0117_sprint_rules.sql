-- Sprint lifecycle rules: known statuses, ordered dates, one active sprint per project,
-- names unique per project ignoring case. Re-runnable.
UPDATE sprints SET status = 'planned'
 WHERE status NOT IN ('planned', 'active', 'completed', 'cancelled');
UPDATE sprints SET end_date = start_date
 WHERE start_date IS NOT NULL AND end_date IS NOT NULL AND end_date < start_date;
UPDATE sprints s SET status = 'planned'
 WHERE s.status = 'active'
   AND EXISTS (SELECT 1 FROM sprints o
                WHERE o.project_id = s.project_id AND o.status = 'active'
                  AND (o.updated_at, o.id) > (s.updated_at, s.id));
UPDATE sprints s SET name = s.name || ' (' || left(s.id::text, 8) || ')'
 WHERE EXISTS (SELECT 1 FROM sprints o
                WHERE o.project_id = s.project_id AND lower(o.name) = lower(s.name)
                  AND (o.created_at, o.id) < (s.created_at, s.id));
ALTER TABLE sprints DROP CONSTRAINT IF EXISTS sprints_project_id_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS sprints_project_lower_name_key ON sprints (project_id, lower(name));
CREATE UNIQUE INDEX IF NOT EXISTS sprints_one_active_per_project ON sprints (project_id) WHERE status = 'active';
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'sprints_status_check') THEN
    ALTER TABLE sprints ADD CONSTRAINT sprints_status_check
      CHECK (status IN ('planned', 'active', 'completed', 'cancelled'));
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'sprints_dates_ordered') THEN
    ALTER TABLE sprints ADD CONSTRAINT sprints_dates_ordered
      CHECK (start_date IS NULL OR end_date IS NULL OR end_date >= start_date);
  END IF;
END $$;
