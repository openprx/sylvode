-- Cap stored record titles at the renderer's limit (follow-up L0.1 F2).
--
-- The single-pass title renderer (`forms::read_policy::render_record_title`, `cap_title`) caps a
-- title at 1,024 characters, but rows written before it may hold longer titles. The renderer is
-- Rust code, so a title cannot be re-rendered here; instead a longer stored title is cut to its
-- first 1,024 characters, which is what `cap_title` keeps. `left()` and `char_length()` count
-- characters (code points) in a UTF-8 database, the unit `cap_title` counts, so the cut always
-- falls on a character boundary. The next write of a record renders its title again.
--
-- Rows only: no object is created, `updated_at` is left alone (the record did not change for its
-- readers' purposes), and the trigger on form_records fires only for writes of `source`.
-- Re-runnable: a second run finds no title longer than the cap and updates nothing.
--
-- Locks: row locks on the rows it rewrites only. A lock timeout keeps a long transaction that
-- holds one of those rows from stalling startup; the migration then fails after the timeout with
-- no partial effect and is retried by the next start (the pattern of 0109).
--
-- Down script: none; the cut text is not kept.

DO $$
BEGIN
  SET LOCAL lock_timeout = '10s';
  IF EXISTS (SELECT 1 FROM form_records WHERE char_length(title) > 1024) THEN
    UPDATE form_records SET title = left(title, 1024) WHERE char_length(title) > 1024;
  END IF;
END $$;
