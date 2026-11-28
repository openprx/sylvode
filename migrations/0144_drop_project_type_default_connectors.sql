-- `project_types.default_connectors` was written by the seed rows of 0025 and 0035 and never read
-- by any code: the connectors it named were retired with the connector tables (0052). The column
-- goes with them.

ALTER TABLE project_types DROP COLUMN IF EXISTS default_connectors;
