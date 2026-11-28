-- Change hints, follow-up to 0132 (audit F5, F6).
--
-- F5: `trg_sv_hint_work_item_update` fired on every UPDATE of every row, including an UPDATE that
-- leaves a row exactly as it was. A burst of such no-op updates overflowed the shared broadcast and
-- ended every open stream with `lagged`. The trigger now fires only when the row actually differs
-- (`OLD IS DISTINCT FROM NEW`); every column of `work_items` has an equality operator, so the
-- whole-row comparison is valid.
--
-- This changes no-op UPDATEs only. A bulk write that really changes rows, including one that
-- only sets `updated_at = now()` (`sprint_moves.rs:81`, `milestone.rs:768`), differs in every
-- row and still sends one notification per row. Those are bounded in the API instead: the hint
-- hub forwards at most a fixed burst of issue hints per workspace per window and turns the rest
-- into one overflow signal (`apps/api/src/events/hints.rs`, `dispatch_hint`).
--
-- F6: the trigger swap takes a short lock on `work_items`; `lock_timeout` is 2 s so a busy table
-- makes the migration fail fast instead of queueing writers behind it for long.
--
-- Re-runnable: the old trigger is dropped and the new one created in the same transaction.

DO $$
BEGIN
  SET LOCAL lock_timeout = '2s';

  DROP TRIGGER IF EXISTS trg_sv_hint_work_item_update ON work_items;
  CREATE TRIGGER trg_sv_hint_work_item_update AFTER UPDATE ON work_items
    FOR EACH ROW WHEN (OLD IS DISTINCT FROM NEW)
    EXECUTE FUNCTION sv_hint_work_item();
END $$;

-- Rollback:
--   DROP TRIGGER IF EXISTS trg_sv_hint_work_item_update ON work_items;
--   CREATE TRIGGER trg_sv_hint_work_item_update AFTER UPDATE ON work_items
--     FOR EACH ROW EXECUTE FUNCTION sv_hint_work_item();
