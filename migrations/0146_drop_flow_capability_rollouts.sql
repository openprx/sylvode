-- The one-time rollout markers of 0106 and 0115 are no longer needed: each migration file runs
-- exactly once, so the "switch on once" guard has nothing left to guard. The column defaults those
-- files set stay in force; the singleton row was switched on when they ran and is not touched
-- again, so an operator's later choice stands. Defaults are not restated here, so later migrations
-- (0128 drops minimum_reader_version) never meet a reference to a dropped column.

DROP TABLE IF EXISTS flow_runtime_capability_rollouts;
