-- Apply this after the actual emitted DDL captured from
-- `postgres_json_extraction_ddl_oracle_fixture_emits_marked_sql`.
-- The PostgreSQL runner wraps this oracle with a completion sentinel and uses
-- a disposable database; the caller wraps execution in a transaction and rolls back all test rows.

TRUNCATE TABLE "documents" RESTART IDENTITY;
TRUNCATE TABLE "required_documents" RESTART IDENTITY;

INSERT INTO "documents" (case_name, payload_json, payload_jsonb) VALUES
(
  'full',
  $json${"model":"Ada \"Q\" \\ path 雪","issue":["Outage","retry"],"items":[{"label":"first"}],"names":["first","last"],"0":"zero-key","customer's name":"apostrophe-key","雪":"unicode-key","escaped":"quote \"double\" and slash \\ 雪","truth":true,"count":42,"nested":{ "b" : 1, "a" : 2, "b" : 3 },"empty":[],"scalar":"text","nil":null}$json$::json,
  $json${"model":"Ada \"Q\" \\ path 雪","issue":["Outage","retry"],"items":[{"label":"first"}],"names":["first","last"],"0":"zero-key","customer's name":"apostrophe-key","雪":"unicode-key","escaped":"quote \"double\" and slash \\ 雪","truth":true,"count":42,"nested":{ "b" : 1, "a" : 2, "b" : 3 },"empty":[],"scalar":"text","nil":null}$json$::jsonb
),
(
  'sql_null',
  NULL,
  NULL
),
(
  'json_null',
  'null'::json,
  'null'::jsonb
),
(
  'wrong_shape',
  '{"model":[],"items":{"label":"not-an-array"},"names":[],"empty":[],"scalar":false}'::json,
  '{"model":[],"items":{"label":"not-an-array"},"names":[],"empty":[],"scalar":false}'::jsonb
),
(
  'missing',
  '{}'::json,
  '{}'::jsonb
),
(
  'index_shape',
  '{"items":[{"label":"only"}],"names":[]}'::json,
  '{"items":[{"label":"only"}],"names":[]}'::jsonb
);

DO $roundhouse_json_oracle$
DECLARE
  full_id bigint;
  required_id bigint;
  before_count bigint;
BEGIN
  IF (SELECT count(DISTINCT case_name) FROM "documents") IS DISTINCT FROM 6 THEN
    RAISE EXCEPTION 'oracle fixture rows are missing or duplicated';
  END IF;
  SELECT id INTO STRICT full_id
    FROM "documents"
    WHERE case_name = 'full';

  IF (SELECT model_json FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM $expected$Ada "Q" \ path 雪$expected$ THEN
    RAISE EXCEPTION 'json string extraction must decode escaped quotes, backslashes, and Unicode';
  END IF;
  IF (SELECT model_jsonb FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM $expected$Ada "Q" \ path 雪$expected$ THEN
    RAISE EXCEPTION 'jsonb string extraction must match json scalar text';
  END IF;
  IF (SELECT issue_first FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'Outage' THEN
    RAISE EXCEPTION 'nested -> followed by positive ->> index mismatch';
  END IF;
  IF (SELECT issue_last_jsonb FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'retry' THEN
    RAISE EXCEPTION 'jsonb nested extraction with negative index mismatch';
  END IF;
  IF (SELECT item_label FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'first' THEN
    RAISE EXCEPTION 'object/array/object extraction chain mismatch';
  END IF;
  IF (SELECT names_first FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'first' THEN
    RAISE EXCEPTION 'positive array index mismatch';
  END IF;
  IF (SELECT names_last FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'last' THEN
    RAISE EXCEPTION 'negative array index mismatch';
  END IF;
  IF (SELECT names_min_index_json FROM "documents" WHERE id = full_id) IS NOT NULL
     OR (SELECT names_min_index_jsonb FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION 'minimum signed int4 array index should be accepted and yield SQL NULL out of range';
  END IF;
  IF (SELECT zero_key FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'zero-key' THEN
    RAISE EXCEPTION 'quoted numeric object key mismatch';
  END IF;
  IF (SELECT zero_index FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION 'integer selector against an object must return SQL NULL';
  END IF;
  IF (SELECT array_text_key FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION 'text selector against an array must return SQL NULL';
  END IF;
  IF (SELECT customer_key FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'apostrophe-key' THEN
    RAISE EXCEPTION 'doubled SQL quote key mismatch';
  END IF;
  IF (SELECT unicode_key FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'unicode-key' THEN
    RAISE EXCEPTION 'Unicode key mismatch';
  END IF;
  IF (SELECT escaped_text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM $expected$quote "double" and slash \ 雪$expected$ THEN
    RAISE EXCEPTION 'escaped JSON string did not produce decoded text';
  END IF;
  IF (SELECT boolean_text FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'true' THEN
    RAISE EXCEPTION 'boolean JSON scalar must extract as lowercase text';
  END IF;
  IF (SELECT number_text FROM "documents" WHERE id = full_id) IS DISTINCT FROM '42' THEN
    RAISE EXCEPTION 'numeric JSON scalar must extract as text';
  END IF;
  IF (SELECT json_nested_text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM '{ "b" : 1, "a" : 2, "b" : 3 }' THEN
    RAISE EXCEPTION 'json ->> object must retain whitespace and duplicate keys';
  END IF;
  IF (SELECT jsonb_nested_text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM '{"a": 2, "b": 3}' THEN
    RAISE EXCEPTION 'jsonb ->> object must normalize whitespace and duplicate keys';
  END IF;
  IF (SELECT pg_typeof(payload_json)::text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM 'json'
     OR (SELECT pg_typeof(payload_jsonb)::text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM 'jsonb' THEN
    RAISE EXCEPTION 'source json/jsonb types were not preserved by emitted DDL';
  END IF;

  IF (SELECT payload_json -> 'nil'::text IS NULL FROM "documents" WHERE id = full_id) THEN
    RAISE EXCEPTION '-> on a found JSON null must return a JSON value, not SQL NULL';
  END IF;
  IF (SELECT (payload_json -> 'nil'::text)::text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM 'null' THEN
    RAISE EXCEPTION '-> must preserve a found JSON null';
  END IF;
  IF (SELECT payload_json ->> 'nil'::text FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION '->> on JSON null must return SQL NULL';
  END IF;
  IF (SELECT payload_json ->> 'missing'::text FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION 'missing JSON key must return SQL NULL';
  END IF;
  IF (SELECT (payload_jsonb -> 'nil'::text)::text FROM "documents" WHERE id = full_id)
       IS DISTINCT FROM 'null' THEN
    RAISE EXCEPTION 'jsonb -> must preserve a found JSON null';
  END IF;
  IF (SELECT payload_jsonb ->> 'nil'::text FROM "documents" WHERE id = full_id) IS NOT NULL THEN
    RAISE EXCEPTION 'jsonb ->> on JSON null must return SQL NULL';
  END IF;

  IF (SELECT missing_text FROM "documents" WHERE case_name = 'sql_null') IS NOT NULL
     OR (SELECT issue_first FROM "documents" WHERE case_name = 'sql_null') IS NOT NULL THEN
    RAISE EXCEPTION 'SQL NULL source must propagate SQL NULL';
  END IF;
  IF (SELECT payload_json IS NULL FROM "documents" WHERE case_name = 'json_null') THEN
    RAISE EXCEPTION 'JSON null source must remain distinct from SQL NULL';
  END IF;
  IF (SELECT model_json FROM "documents" WHERE case_name = 'json_null') IS NOT NULL THEN
    RAISE EXCEPTION 'extraction from JSON null must return SQL NULL';
  END IF;
  IF (SELECT missing_text FROM "documents" WHERE case_name = 'missing') IS NOT NULL
     OR (SELECT zero_index FROM "documents" WHERE case_name = 'missing') IS NOT NULL THEN
    RAISE EXCEPTION 'missing keys and out-of-shape index access must return SQL NULL';
  END IF;
  IF (SELECT item_label FROM "documents" WHERE case_name = 'wrong_shape') IS NOT NULL
     OR (SELECT names_last FROM "documents" WHERE case_name = 'wrong_shape') IS NOT NULL
     OR (SELECT text_selector_array FROM "documents" WHERE case_name = 'wrong_shape') IS NOT NULL
     OR (SELECT empty_negative_index FROM "documents" WHERE case_name = 'wrong_shape') IS NOT NULL
     OR (SELECT scalar_descent FROM "documents" WHERE case_name = 'wrong_shape') IS NOT NULL THEN
    RAISE EXCEPTION 'wrong structure, empty negative index, and scalar descent must return SQL NULL';
  END IF;
  IF (SELECT names_last FROM "documents" WHERE case_name = 'index_shape') IS NOT NULL THEN
    RAISE EXCEPTION 'negative index on an empty array must return SQL NULL';
  END IF;

  UPDATE "documents"
     SET payload_json = '{"model":"Updated","issue":["Changed"],"items":[{"label":"updated-item"}],"names":["new-first","new-last"]}'::json,
         payload_jsonb = '{"model":"Updated","issue":["Changed"],"items":[{"label":"updated-item"}],"names":["new-first","new-last"]}'::jsonb
   WHERE id = full_id;
  IF (SELECT model_json FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'Updated'
     OR (SELECT issue_first FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'Changed'
     OR (SELECT item_label FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'updated-item'
     OR (SELECT names_last FROM "documents" WHERE id = full_id) IS DISTINCT FROM 'new-last' THEN
    RAISE EXCEPTION 'stored extraction columns did not recompute after UPDATE';
  END IF;

  INSERT INTO "required_documents" (payload_json)
    VALUES ('{"model":"kept"}'::json)
    RETURNING id INTO STRICT required_id;
  IF (SELECT required_model FROM "required_documents" WHERE id = required_id)
       IS DISTINCT FROM 'kept' THEN
    RAISE EXCEPTION 'emitted NOT NULL generated column did not store its value';
  END IF;

  SELECT count(*) INTO before_count FROM "required_documents";
  BEGIN
    INSERT INTO "required_documents" (payload_json) VALUES ('{}'::json);
    RAISE EXCEPTION 'NOT NULL generated output unexpectedly accepted a missing key';
  EXCEPTION WHEN not_null_violation THEN
    NULL;
  END;
  IF (SELECT count(*) FROM "required_documents") IS DISTINCT FROM before_count THEN
    RAISE EXCEPTION 'failed NOT NULL insert persisted a row';
  END IF;

  BEGIN
    UPDATE "required_documents" SET payload_json = '{}'::json WHERE id = required_id;
    RAISE EXCEPTION 'NOT NULL generated output unexpectedly accepted a missing key on UPDATE';
  EXCEPTION WHEN not_null_violation THEN
    NULL;
  END;
  IF (SELECT required_model FROM "required_documents" WHERE id = required_id)
       IS DISTINCT FROM 'kept'
     OR (SELECT payload_json ->> 'model'::text FROM "required_documents" WHERE id = required_id)
       IS DISTINCT FROM 'kept' THEN
    RAISE EXCEPTION 'failed NOT NULL UPDATE did not roll back the source row';
  END IF;
END;
$roundhouse_json_oracle$;
