-- Change hints of package upload drafts (audit M11 F1). `sv_hint_draft` (0139) sent `"p": null`
-- for a draft without a project, which the hint parser could not read: the listener answered with a
-- reset that ended every bot hint stream of the instance. A `package_upload` draft now sends a hint
-- that names its submitting bot (`b`), so only that bot's streams are woken; a draft without a
-- project and without a bot author sends no hint at all.
--
-- Re-runnable: `CREATE OR REPLACE` leaves the triggers of 0139 in place and they call it by name.

CREATE OR REPLACE FUNCTION sv_hint_draft() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    IF NEW.project_id IS NULL AND NEW.author_kind <> 'bot' THEN
      RETURN NEW;
    END IF;
    PERFORM pg_notify('sv_change_hint', jsonb_build_object(
      'k', 'draft', 'id', NEW.id, 'p', NEW.project_id, 'w', NEW.workspace_id,
      'b', CASE WHEN NEW.project_id IS NULL THEN NEW.author_id END)::text);
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a draft (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;
