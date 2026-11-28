-- Change hints, follow-up to 0132 and 0143 (RT-C C1): creating a work item sends a change hint.
--
-- `trg_sv_hint_work_item_update` and `trg_sv_hint_work_item_delete` cover UPDATE and DELETE only, so a
-- bot that watches a project never learned about a new work item until something changed it. The new
-- AFTER INSERT trigger runs the same function as the UPDATE trigger (`sv_hint_work_item()` builds
-- `{"k":"issue","id":...,"p":...,"w":...}` for every operation but DELETE from NEW), so the payload is the
-- same. An INSERT has no OLD row, so there is no "differs" condition.
--
-- `lock_timeout` is 2 s like in 0143: the trigger creation takes a short lock on `work_items`.
-- Re-runnable: the trigger is dropped and created again in the same transaction.

DO $$
BEGIN
  SET LOCAL lock_timeout = '2s';

  DROP TRIGGER IF EXISTS trg_sv_hint_work_item_insert ON work_items;
  CREATE TRIGGER trg_sv_hint_work_item_insert AFTER INSERT ON work_items
    FOR EACH ROW EXECUTE FUNCTION sv_hint_work_item();
END $$;

-- Rollback:
--   DROP TRIGGER IF EXISTS trg_sv_hint_work_item_insert ON work_items;
