-- Public sites (contract C3 section 6.5): a site per project, its pages (layouts in site context),
-- its form bindings with their visitor/operator projections, operator roles, anonymous visitors
-- and operator sessions. Every statement is safe to run again.

CREATE TABLE IF NOT EXISTS sites (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  slug TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,39}$'),
  title JSONB NOT NULL,
  settings JSONB NOT NULL DEFAULT '{}'::jsonb,
  status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft','published','disabled')),
  created_by UUID NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS sites_project ON sites (project_id);

CREATE TABLE IF NOT EXISTS site_pages (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  route TEXT NOT NULL CHECK (route ~ '^/([a-z0-9-]+(/[a-z0-9-]+|/:[a-z_]+)*)?$'),
  layout_id UUID NOT NULL REFERENCES layouts(id),
  audience TEXT NOT NULL CHECK (audience IN ('visitor','operator')),
  nav JSONB NULL,
  position INTEGER NOT NULL DEFAULT 0,
  installation_id UUID NULL,
  generation INTEGER NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS site_pages_route ON site_pages (site_id, route, COALESCE(generation, 0));
CREATE INDEX IF NOT EXISTS site_pages_layout ON site_pages (layout_id);

CREATE TABLE IF NOT EXISTS site_bindings (
  site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  binding_key TEXT NOT NULL CHECK (binding_key ~ '^[a-z][a-z0-9_]{0,39}$'),
  form_id UUID NOT NULL REFERENCES project_forms(id),
  projection JSONB NOT NULL,
  PRIMARY KEY (site_id, binding_key)
);

CREATE TABLE IF NOT EXISTS site_roles (
  site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('site_operator','site_editor')),
  PRIMARY KEY (site_id, user_id)
);

CREATE TABLE IF NOT EXISTS site_visitors (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  token_hash BYTEA NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (site_id, token_hash)
);

-- `session_version` is the user's `users.session_version` at login: a password change or a
-- deactivation bumps it, which ends every operator session exactly like a console session.
CREATE TABLE IF NOT EXISTS site_sessions (
  token_hash BYTEA PRIMARY KEY,
  site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  session_version BIGINT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS site_sessions_user ON site_sessions (user_id);

CREATE INDEX IF NOT EXISTS form_records_site_visitor
  ON form_records ((source->>'site_visitor_id')) WHERE source->>'kind' = 'site';

-- A record a visitor submitted stays theirs: every writer replaces `form_records.source` with its
-- own, so the site origin (`kind`, `site_id`, `site_visitor_id`) is carried over on each update.
-- Events keep the writer's own source; only the stored record keeps the origin.
CREATE OR REPLACE FUNCTION sv_site_record_origin() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF jsonb_typeof(NEW.source) = 'object' THEN
    NEW.source := NEW.source || jsonb_build_object(
      'kind', 'site',
      'site_id', OLD.source->'site_id',
      'site_visitor_id', OLD.source->'site_visitor_id');
  ELSE
    NEW.source := jsonb_build_object(
      'kind', 'site',
      'site_id', OLD.source->'site_id',
      'site_visitor_id', OLD.source->'site_visitor_id');
  END IF;
  RETURN NEW;
END $$;

DO $$
BEGIN
  SET LOCAL lock_timeout = '10s';
  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('form_records') AND tgname = 'trg_sv_site_record_origin'
                   AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_sv_site_record_origin BEFORE UPDATE OF source ON form_records
      FOR EACH ROW
      WHEN (OLD.source->>'kind' = 'site' AND OLD.source ? 'site_visitor_id'
            AND NEW.source IS DISTINCT FROM OLD.source)
      EXECUTE FUNCTION sv_site_record_origin();
  END IF;
END $$;
