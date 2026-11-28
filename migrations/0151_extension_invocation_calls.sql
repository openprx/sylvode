-- Plugin host call audit (design M10 §3.6): one row per write or destructive catalogue call of an
-- invocation, no inputs and no outputs; reads are only counted. Rows go with their invocation
-- (30-day retention, `extensions::host::log::purge_old`).
ALTER TABLE extension_invocations ADD COLUMN IF NOT EXISTS read_calls INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS extension_invocation_calls (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  invocation_id UUID NOT NULL REFERENCES extension_invocations(id) ON DELETE CASCADE,
  installation_id UUID NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  call_index INTEGER NOT NULL CHECK (call_index > 0),
  capability TEXT NOT NULL CHECK (capability ~ '^[a-z0-9_.]{1,128}$'),
  op TEXT NOT NULL CHECK (op ~ '^[a-z0-9_./-]{1,160}$'),
  method TEXT NOT NULL CHECK (method IN ('GET','POST','PUT','PATCH','DELETE')),
  risk TEXT NOT NULL CHECK (risk IN ('write','destructive')),
  -- Path parameter ids of the target (name -> canonical UUID); never body or query values.
  target_ids JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(target_ids) = 'object'),
  project_id UUID,
  outcome TEXT NOT NULL CHECK (outcome IN ('ok','denied','not_found','invalid','conflict','limit','unavailable')),
  error_code TEXT CHECK (error_code ~ '^[a-z0-9_.]{1,128}$'),
  duration_ms INTEGER NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS extension_invocation_calls_installation
  ON extension_invocation_calls (installation_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS extension_invocation_calls_invocation
  ON extension_invocation_calls (invocation_id);
