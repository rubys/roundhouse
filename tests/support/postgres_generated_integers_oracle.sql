-- Apply this after the exact emitted DDL captured from
-- `postgres_generated_int4_ddl_oracle_fixture_emits_marked_sql`. The runner
-- must wrap emitted DDL and these assertions in BEGIN/ROLLBACK on a disposable database.


INSERT INTO "counters" (case_name, payload_json, payload_jsonb, counter_text) VALUES
  ('zero', '{"counter":"0"}'::json, '{"counter":"0"}'::jsonb, '0'),
  ('negative', '{"counter":"-17"}'::json, '{"counter":"-17"}'::jsonb, '-17'),
  ('minimum', '{"counter":"-2147483648"}'::json, '{"counter":"-2147483648"}'::jsonb, '-2147483648'),
  ('maximum', '{"counter":"2147483647"}'::json, '{"counter":"2147483647"}'::jsonb, '2147483647'),
  ('number', '{"counter":42}'::json, '{"counter":42}'::jsonb, '42'),
  ('sql_null', NULL, NULL, NULL),
  ('json_null', 'null'::json, 'null'::jsonb, NULL),
  ('key_null', '{"counter":null}'::json, '{"counter":null}'::jsonb, NULL),
  ('missing', '{}'::json, '{}'::jsonb, NULL),
  ('wrong_shape', '[]'::json, '[]'::jsonb, NULL),
  ('recompute', '{"counter":"1"}'::json, '{"counter":"1"}'::jsonb, '1'),
  ('error_probe', '{"counter":"1"}'::json, '{"counter":"1"}'::jsonb, '1');

DO $roundhouse_int4_oracle$
DECLARE
  actual_state text;
  source_name text;
  bad_value text;
  json_shape_expression text;
  sql_statement text;
  required_id bigint;
  before_count bigint;
BEGIN
  IF (SELECT count(*) FROM "counters") IS DISTINCT FROM 12 THEN
    RAISE EXCEPTION 'int4 oracle fixture rows are missing or duplicated';
  END IF;

  IF EXISTS (
    SELECT 1
      FROM (VALUES
        ('zero', 0),
        ('negative', -17),
        ('minimum', -2147483648),
        ('maximum', 2147483647),
        ('number', 42)
      ) AS expected(case_name, expected_value)
      JOIN "counters" USING (case_name)
     WHERE from_json IS DISTINCT FROM expected.expected_value
        OR from_jsonb IS DISTINCT FROM expected.expected_value
        OR from_text IS DISTINCT FROM expected.expected_value
  ) THEN
    RAISE EXCEPTION 'text-to-int4 generated values differ at valid values or endpoints';
  END IF;

  IF (SELECT pg_typeof(from_json)::text FROM "counters" WHERE case_name = 'minimum')
       IS DISTINCT FROM 'integer'
     OR (SELECT pg_typeof(from_jsonb)::text FROM "counters" WHERE case_name = 'minimum')
       IS DISTINCT FROM 'integer'
     OR (SELECT pg_typeof(from_text)::text FROM "counters" WHERE case_name = 'minimum')
       IS DISTINCT FROM 'integer' THEN
    RAISE EXCEPTION 'generated expression outputs are not PostgreSQL int4';
  END IF;

  IF (SELECT pg_typeof(payload_json)::text FROM "counters" WHERE case_name = 'zero')
       IS DISTINCT FROM 'json'
     OR (SELECT pg_typeof(payload_jsonb)::text FROM "counters" WHERE case_name = 'zero')
       IS DISTINCT FROM 'jsonb' THEN
    RAISE EXCEPTION 'source json/jsonb types were not retained in emitted DDL';
  END IF;

  IF (SELECT count(*) FROM "counters"
       WHERE case_name IN ('sql_null', 'json_null', 'key_null', 'missing', 'wrong_shape'))
       IS DISTINCT FROM 5 THEN
    RAISE EXCEPTION 'nullable-shape fixture rows are missing';
  END IF;
  IF EXISTS (
    SELECT 1 FROM "counters"
     WHERE case_name IN ('sql_null', 'json_null', 'key_null', 'missing', 'wrong_shape')
       AND (from_json IS NOT NULL OR from_jsonb IS NOT NULL OR from_text IS NOT NULL)
  ) THEN
    RAISE EXCEPTION 'SQL NULL, JSON null, missing key, and wrong shape must yield NULL';
  END IF;

  UPDATE "counters"
     SET payload_json = '{"counter":"-37"}'::json,
         payload_jsonb = '{"counter":"-37"}'::jsonb,
         counter_text = '-37'
   WHERE case_name = 'recompute';
  IF (SELECT from_json FROM "counters" WHERE case_name = 'recompute') IS DISTINCT FROM -37
     OR (SELECT from_jsonb FROM "counters" WHERE case_name = 'recompute') IS DISTINCT FROM -37
     OR (SELECT from_text FROM "counters" WHERE case_name = 'recompute') IS DISTINCT FROM -37 THEN
    RAISE EXCEPTION 'stored int4 generated values did not recompute on UPDATE';
  END IF;

  -- Invalid text fails through PostgreSQL's int4 input function. Exercise all
  -- three source columns and ensure each failed UPDATE leaves its row intact.
  FOREACH source_name IN ARRAY ARRAY['payload_json', 'payload_jsonb', 'counter_text'] LOOP
    FOREACH bad_value IN ARRAY ARRAY['abc', '42x', '123e+5', '', 'true', '{}', '[]'] LOOP
      IF source_name = 'payload_json' THEN
        sql_statement := format(
          'UPDATE "counters" SET "payload_json" = json_build_object(''counter'', %L)::json WHERE "case_name" = ''error_probe''',
          bad_value
        );
      ELSIF source_name = 'payload_jsonb' THEN
        sql_statement := format(
          'UPDATE "counters" SET "payload_jsonb" = jsonb_build_object(''counter'', %L)::jsonb WHERE "case_name" = ''error_probe''',
          bad_value
        );
      ELSE
        sql_statement := format(
          'UPDATE "counters" SET "counter_text" = %L WHERE "case_name" = ''error_probe''',
          bad_value
        );
      END IF;

      actual_state := NULL;
      BEGIN
        EXECUTE sql_statement;
      EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
      END;
      IF actual_state IS DISTINCT FROM '22P02' THEN
        RAISE EXCEPTION 'invalid int4 text % from % returned SQLSTATE %, expected 22P02',
          bad_value, source_name, actual_state;
      END IF;
      IF (SELECT payload_json ->> 'counter'::text FROM "counters" WHERE case_name = 'error_probe')
           IS DISTINCT FROM '1'
         OR (SELECT payload_jsonb ->> 'counter'::text FROM "counters" WHERE case_name = 'error_probe')
           IS DISTINCT FROM '1'
         OR (SELECT counter_text FROM "counters" WHERE case_name = 'error_probe')
           IS DISTINCT FROM '1'
         OR (SELECT from_json FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1
         OR (SELECT from_jsonb FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1
         OR (SELECT from_text FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'failed invalid-text UPDATE changed the source row or generated values';
      END IF;
    END LOOP;
  END LOOP;

  -- A JSON boolean/object/array under the selected key extracts as text, then
  -- fails the int4 cast. Also cover each JSON storage type with actual shapes.
  FOREACH source_name IN ARRAY ARRAY['payload_json', 'payload_jsonb'] LOOP
    FOREACH json_shape_expression IN ARRAY ARRAY[
      'json_build_object(''counter'', true)',
      'json_build_object(''counter'', json_build_object(''n'', 1))',
      'json_build_object(''counter'', json_build_array(1))'
    ] LOOP
      IF source_name = 'payload_json' THEN
        sql_statement := format(
          'UPDATE "counters" SET "payload_json" = %s::json WHERE "case_name" = ''error_probe''',
          json_shape_expression
        );
      ELSE
        sql_statement := format(
          'UPDATE "counters" SET "payload_jsonb" = %s::jsonb WHERE "case_name" = ''error_probe''',
          replace(json_shape_expression, 'json_build_', 'jsonb_build_')
        );
      END IF;
      actual_state := NULL;
      BEGIN
        EXECUTE sql_statement;
      EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
      END;
      IF actual_state IS DISTINCT FROM '22P02' THEN
        RAISE EXCEPTION 'JSON non-string scalar/container from % returned SQLSTATE %, expected 22P02',
          source_name, actual_state;
      END IF;
      IF (SELECT from_json FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1
         OR (SELECT from_jsonb FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'failed JSON-shape UPDATE changed generated values';
      END IF;
    END LOOP;
  END LOOP;

  FOREACH source_name IN ARRAY ARRAY['payload_json', 'payload_jsonb', 'counter_text'] LOOP
    FOREACH bad_value IN ARRAY ARRAY['2147483648', '-2147483649'] LOOP
      IF source_name = 'payload_json' THEN
        sql_statement := format(
          'UPDATE "counters" SET "payload_json" = json_build_object(''counter'', %L)::json WHERE "case_name" = ''error_probe''',
          bad_value
        );
      ELSIF source_name = 'payload_jsonb' THEN
        sql_statement := format(
          'UPDATE "counters" SET "payload_jsonb" = jsonb_build_object(''counter'', %L)::jsonb WHERE "case_name" = ''error_probe''',
          bad_value
        );
      ELSE
        sql_statement := format(
          'UPDATE "counters" SET "counter_text" = %L WHERE "case_name" = ''error_probe''',
          bad_value
        );
      END IF;

      actual_state := NULL;
      BEGIN
        EXECUTE sql_statement;
      EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
      END;
      IF actual_state IS DISTINCT FROM '22003' THEN
        RAISE EXCEPTION 'out-of-range int4 text % from % returned SQLSTATE %, expected 22003',
          bad_value, source_name, actual_state;
      END IF;
      IF (SELECT from_json FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1
         OR (SELECT from_jsonb FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1
         OR (SELECT from_text FROM "counters" WHERE case_name = 'error_probe') IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'failed out-of-range UPDATE changed generated values';
      END IF;
    END LOOP;
  END LOOP;

  SELECT count(*) INTO before_count FROM "counters";
  actual_state := NULL;
  BEGIN
    INSERT INTO "counters" (case_name, payload_jsonb)
      VALUES ('invalid_insert', '{"counter":"abc"}'::jsonb);
  EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
  END;
  IF actual_state IS DISTINCT FROM '22P02' THEN
    RAISE EXCEPTION 'invalid-text insert returned SQLSTATE %, expected 22P02', actual_state;
  END IF;
  IF (SELECT count(*) FROM "counters") IS DISTINCT FROM before_count
     OR EXISTS (SELECT 1 FROM "counters" WHERE case_name = 'invalid_insert') THEN
    RAISE EXCEPTION 'failed invalid-text INSERT persisted a row';
  END IF;

  actual_state := NULL;
  BEGIN
    INSERT INTO "counters" (case_name, payload_jsonb)
      VALUES ('range_insert', '{"counter":"2147483648"}'::jsonb);
  EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
  END;
  IF actual_state IS DISTINCT FROM '22003' THEN
    RAISE EXCEPTION 'out-of-range insert returned SQLSTATE %, expected 22003', actual_state;
  END IF;
  IF (SELECT count(*) FROM "counters") IS DISTINCT FROM before_count
     OR EXISTS (SELECT 1 FROM "counters" WHERE case_name = 'range_insert') THEN
    RAISE EXCEPTION 'failed out-of-range INSERT persisted a row';
  END IF;

  INSERT INTO "required_counters" (payload_jsonb)
    VALUES ('{"counter":"123"}'::jsonb)
    RETURNING id INTO STRICT required_id;
  IF (SELECT required_count FROM "required_counters" WHERE id = required_id) IS DISTINCT FROM 123
     OR (SELECT pg_typeof(required_count)::text FROM "required_counters" WHERE id = required_id)
       IS DISTINCT FROM 'integer' THEN
    RAISE EXCEPTION 'NOT NULL generated int4 value was not stored with int4 type';
  END IF;

  SELECT count(*) INTO before_count FROM "required_counters";
  actual_state := NULL;
  BEGIN
    INSERT INTO "required_counters" (payload_jsonb) VALUES ('{}'::jsonb);
  EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
  END;
  IF actual_state IS DISTINCT FROM '23502' THEN
    RAISE EXCEPTION 'missing-key NOT NULL insert returned SQLSTATE %, expected 23502', actual_state;
  END IF;
  IF (SELECT count(*) FROM "required_counters") IS DISTINCT FROM before_count THEN
    RAISE EXCEPTION 'failed NOT NULL insert persisted a row';
  END IF;

  actual_state := NULL;
  BEGIN
    UPDATE "required_counters" SET payload_jsonb = '{}'::jsonb WHERE id = required_id;
  EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS actual_state = RETURNED_SQLSTATE;
  END;
  IF actual_state IS DISTINCT FROM '23502' THEN
    RAISE EXCEPTION 'missing-key NOT NULL update returned SQLSTATE %, expected 23502', actual_state;
  END IF;
  IF (SELECT payload_jsonb ->> 'counter'::text FROM "required_counters" WHERE id = required_id)
       IS DISTINCT FROM '123'
     OR (SELECT required_count FROM "required_counters" WHERE id = required_id) IS DISTINCT FROM 123 THEN
    RAISE EXCEPTION 'failed NOT NULL UPDATE persisted its source or generated value';
  END IF;
END;
$roundhouse_int4_oracle$;
