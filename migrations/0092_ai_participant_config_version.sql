-- Configuration revision for optimistic project-assistant editing. Heartbeat activity is separate.
ALTER TABLE ai_participants ADD COLUMN IF NOT EXISTS config_updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
CREATE OR REPLACE FUNCTION touch_ai_participant_config_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 NEW.config_updated_at:=GREATEST(clock_timestamp(),OLD.config_updated_at+interval '1 microsecond');
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS ai_participant_config_version ON ai_participants;
-- UPDATE OF deliberately advances even an accepted no-op configuration PATCH. A heartbeat that
-- only writes last_active_at does not run this trigger and cannot invalidate an editing draft.
CREATE TRIGGER ai_participant_config_version BEFORE UPDATE OF
 name,model,provider,api_endpoint,capabilities,domain_overrides,max_domain_level,
 can_veto_human_consensus,reason_min_length,is_active
 ON ai_participants FOR EACH ROW EXECUTE FUNCTION touch_ai_participant_config_version();
