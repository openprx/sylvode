-- Form schema v2 (low-code programme, contract C2 §2.1). One-way rewrite of every stored form
-- schema from `openpr.form.schema.v1` to `sv.form.schema.v2`:
--
--   validation.min / .max     -> rules.validate [{"rule":"min"|"max","value":...}]
--                                integer, rating, progress: JSON integer; number, amount: decimal
--                                string; date: "YYYY-MM-DD"; datetime: RFC 3339 (a zone-less
--                                browser value gets seconds and "Z")
--   validation.pattern        -> {"rule":"pattern","value":"^(?:<pattern>)$"} on text-like fields
--                                (v1 patterns matched the whole value; v2 matching is unanchored)
--   validation.max_length     -> {"rule":"max_length","value":<int>} on text-like and rich text fields
--   validation on a type the rule does not fit, or an unparsable bound -> dropped and counted
--   default_value (non-null)  -> "default": {"value": <same json>}
--   policy.read_only = true   -> "read_only": true; "policy" is removed
--
-- Applies to project_forms.schema, form_schema_versions.schema (so a restored version is v2),
-- the form bundles in scenario_templates.sample_data.forms and the scenario template field
-- schemas forms are created from. Idempotent: only schemas whose version is not v2 are touched.
-- Patterns and defaults are not compiled here; after deploy every stored schema is checked with
-- POST /api/v1/forms/schema/validate, and an entry that does not hold is skipped on write.

-- The dropped-rule counter is its own table: the converter functions run inside the UPDATE
-- statements below and bump it, and a statement that then updates the same row (the row count
-- tables) fails with "tuple to be updated was already modified by an operation triggered by the
-- current command".
CREATE TEMP TABLE sv_0136_counts (
  forms INTEGER NOT NULL,
  versions INTEGER NOT NULL,
  templates INTEGER NOT NULL
) ON COMMIT DROP;

INSERT INTO sv_0136_counts VALUES (0, 0, 0);

CREATE TEMP TABLE sv_0136_dropped (
  dropped INTEGER NOT NULL
) ON COMMIT DROP;

INSERT INTO sv_0136_dropped VALUES (0);

CREATE OR REPLACE FUNCTION sv_form_field_v1_to_v2(field JSONB) RETURNS JSONB LANGUAGE plpgsql AS $$
DECLARE
  ftype TEXT := COALESCE(NULLIF(btrim(field->>'type'), ''), 'text');
  validation JSONB := field->'validation';
  result JSONB := field - 'validation' - 'default_value' - 'policy';
  new_rules JSONB := '[]'::JSONB;
  dropped_count INTEGER := 0;
  bound_key TEXT;
  raw JSONB;
  raw_text TEXT;
  rule_value JSONB;
  existing_validate JSONB;
  length_ok BOOLEAN;
  text_types CONSTANT TEXT[] := ARRAY['text', 'textarea', 'phone', 'email', 'address', 'scan'];
BEGIN
  IF validation IS NOT NULL AND jsonb_typeof(validation) = 'object' THEN
    FOREACH bound_key IN ARRAY ARRAY['min', 'max'] LOOP
      raw := validation->bound_key;
      CONTINUE WHEN raw IS NULL OR jsonb_typeof(raw) = 'null'
        OR (jsonb_typeof(raw) = 'string' AND btrim(raw #>> '{}') = '');
      raw_text := btrim(raw #>> '{}');
      rule_value := NULL;
      IF jsonb_typeof(raw) IN ('number', 'string') THEN
        IF ftype IN ('integer', 'rating', 'progress') THEN
          IF raw_text ~ '^-?[0-9]{1,15}(\.0+)?$' THEN
            rule_value := to_jsonb(split_part(raw_text, '.', 1)::BIGINT);
          END IF;
        ELSIF ftype IN ('number', 'amount') THEN
          IF raw_text ~ '^-?[0-9]+(\.[0-9]+)?$' OR raw_text ~ '^-?[0-9]+(\.[0-9]+)?[eE][-+]?[0-9]{1,3}$' THEN
            rule_value := to_jsonb(raw_text::NUMERIC::TEXT);
          END IF;
        ELSIF ftype = 'date' AND jsonb_typeof(raw) = 'string' THEN
          IF raw_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$' THEN
            rule_value := to_jsonb(raw_text);
          END IF;
        ELSIF ftype = 'datetime' AND jsonb_typeof(raw) = 'string' THEN
          IF raw_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$' THEN
            rule_value := to_jsonb(raw_text);
          ELSIF raw_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}$' THEN
            rule_value := to_jsonb(raw_text || 'Z');
          ELSIF raw_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}$' THEN
            rule_value := to_jsonb(raw_text || ':00Z');
          ELSIF raw_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$' THEN
            rule_value := to_jsonb(raw_text || 'T00:00:00Z');
          END IF;
        END IF;
      END IF;
      IF rule_value IS NULL THEN
        dropped_count := dropped_count + 1;
      ELSE
        new_rules := new_rules || jsonb_build_array(jsonb_build_object('rule', bound_key, 'value', rule_value));
      END IF;
    END LOOP;

    raw := validation->'pattern';
    IF raw IS NOT NULL AND jsonb_typeof(raw) <> 'null' AND NOT (jsonb_typeof(raw) = 'string' AND raw #>> '{}' = '') THEN
      IF jsonb_typeof(raw) = 'string' AND ftype = ANY (text_types) THEN
        new_rules := new_rules || jsonb_build_array(
          jsonb_build_object('rule', 'pattern', 'value', '^(?:' || (raw #>> '{}') || ')$'));
      ELSE
        dropped_count := dropped_count + 1;
      END IF;
    END IF;

    raw := validation->'max_length';
    IF raw IS NOT NULL AND jsonb_typeof(raw) <> 'null' THEN
      raw_text := btrim(raw #>> '{}');
      -- Decided before the IF: PL/pgSQL ends an IF condition at the first THEN, so a CASE
      -- expression cannot appear inside one.
      length_ok := raw_text ~ '^[0-9]{1,7}$';
      IF length_ok THEN
        length_ok := raw_text::INTEGER BETWEEN 1 AND 1048576;
      END IF;
      IF (ftype = ANY (text_types) OR ftype = 'rich_text')
         AND jsonb_typeof(raw) IN ('number', 'string')
         AND length_ok THEN
        new_rules := new_rules || jsonb_build_array(
          jsonb_build_object('rule', 'max_length', 'value', raw_text::INTEGER));
      ELSE
        dropped_count := dropped_count + 1;
      END IF;
    END IF;

    dropped_count := dropped_count + (
      SELECT count(*)::INTEGER FROM jsonb_object_keys(validation) AS k(name)
      WHERE k.name NOT IN ('min', 'max', 'pattern', 'max_length')
    );
  ELSIF validation IS NOT NULL AND jsonb_typeof(validation) <> 'null' THEN
    dropped_count := dropped_count + 1;
  END IF;

  IF jsonb_array_length(new_rules) > 0 THEN
    existing_validate := CASE
      WHEN jsonb_typeof(result #> '{rules,validate}') = 'array' THEN result #> '{rules,validate}'
      ELSE '[]'::JSONB
    END;
    result := jsonb_set(
      result,
      '{rules}',
      CASE WHEN jsonb_typeof(result->'rules') = 'object' THEN result->'rules' ELSE '{}'::JSONB END
        || jsonb_build_object('validate', existing_validate || new_rules)
    );
  END IF;

  IF field ? 'default_value' AND jsonb_typeof(field->'default_value') <> 'null' THEN
    result := jsonb_set(result, '{default}', jsonb_build_object('value', field->'default_value'));
  END IF;

  IF jsonb_typeof(field->'policy') = 'object' AND (field #> '{policy,read_only}') = 'true'::JSONB THEN
    result := jsonb_set(result, '{read_only}', 'true'::JSONB);
  END IF;

  IF dropped_count > 0 THEN
    UPDATE sv_0136_dropped SET dropped = sv_0136_dropped.dropped + dropped_count;
  END IF;
  RETURN result;
END
$$;

CREATE OR REPLACE FUNCTION sv_form_schema_v1_to_v2(doc JSONB) RETURNS JSONB LANGUAGE plpgsql AS $$
DECLARE
  new_fields JSONB;
BEGIN
  IF doc IS NULL OR jsonb_typeof(doc) <> 'object' THEN
    RETURN doc;
  END IF;
  IF jsonb_typeof(doc->'fields') = 'array' THEN
    SELECT COALESCE(
      jsonb_agg(
        CASE WHEN jsonb_typeof(item.field) = 'object' THEN sv_form_field_v1_to_v2(item.field) ELSE item.field END
        ORDER BY item.ord
      ),
      '[]'::JSONB
    )
    INTO new_fields
    FROM jsonb_array_elements(doc->'fields') WITH ORDINALITY AS item(field, ord);
    doc := jsonb_set(doc, '{fields}', new_fields);
  END IF;
  RETURN jsonb_set(doc, '{version}', '"sv.form.schema.v2"'::JSONB);
END
$$;

WITH changed AS (
  UPDATE project_forms
     SET schema = sv_form_schema_v1_to_v2(schema)
   WHERE schema->>'version' IS DISTINCT FROM 'sv.form.schema.v2'
  RETURNING 1
)
UPDATE sv_0136_counts SET forms = (SELECT count(*) FROM changed);

WITH changed AS (
  UPDATE form_schema_versions
     SET schema = sv_form_schema_v1_to_v2(schema)
   WHERE schema->>'version' IS DISTINCT FROM 'sv.form.schema.v2'
  RETURNING 1
)
UPDATE sv_0136_counts SET versions = (SELECT count(*) FROM changed);

WITH changed AS (
  UPDATE scenario_templates
     SET sample_data = jsonb_set(
           sample_data,
           '{forms}',
           (
             SELECT jsonb_agg(
                      CASE
                        WHEN jsonb_typeof(item.form->'schema') = 'object'
                             AND item.form->'schema'->>'version' IS DISTINCT FROM 'sv.form.schema.v2'
                          THEN jsonb_set(item.form, '{schema}', sv_form_schema_v1_to_v2(item.form->'schema'))
                        ELSE item.form
                      END
                      ORDER BY item.ord
                    )
               FROM jsonb_array_elements(sample_data->'forms') WITH ORDINALITY AS item(form, ord)
           )
         )
   WHERE jsonb_typeof(sample_data->'forms') = 'array'
     AND EXISTS (
           SELECT 1
             FROM jsonb_array_elements(sample_data->'forms') AS item(form)
            WHERE jsonb_typeof(item.form->'schema') = 'object'
              AND item.form->'schema'->>'version' IS DISTINCT FROM 'sv.form.schema.v2'
         )
  RETURNING 1
)
UPDATE sv_0136_counts SET templates = (SELECT count(*) FROM changed);

WITH changed AS (
  UPDATE scenario_templates
     SET field_schema = sv_form_schema_v1_to_v2(field_schema)
   WHERE jsonb_typeof(field_schema->'fields') = 'array'
     AND field_schema->>'version' IS DISTINCT FROM 'sv.form.schema.v2'
  RETURNING 1
)
UPDATE sv_0136_counts SET templates = templates + (SELECT count(*) FROM changed);

DO $$
DECLARE
  counts RECORD;
  dropped_rules INTEGER;
BEGIN
  SELECT * INTO counts FROM sv_0136_counts;
  SELECT dropped INTO dropped_rules FROM sv_0136_dropped;
  RAISE NOTICE 'form schema v2: % forms, % schema versions, % scenario templates rewritten; % validation rules dropped',
    counts.forms, counts.versions, counts.templates, dropped_rules;
END
$$;

DROP FUNCTION sv_form_schema_v1_to_v2(JSONB);
DROP FUNCTION sv_form_field_v1_to_v2(JSONB);
