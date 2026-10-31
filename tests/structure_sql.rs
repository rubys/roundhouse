//! `db/structure.sql` — the pg_dump-shaped fallback schema source for
//! apps with `schema_format: :sql` (no `db/schema.rb`). See
//! `src/ingest/structure_sql.rs` for the parser and
//! `docs/data/schema-routes-seeds.md` for how this fits alongside the
//! schema.rb / migration-fold sources.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use roundhouse::ingest::structure_sql::ingest_structure_sql;
use roundhouse::ingest::{ingest_app_from_tree, survey};
use roundhouse::schema::ColumnType;
use roundhouse::{Symbol, Ty};

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec())).collect()
}

/// A small hand-rolled `pg_dump`-shaped dump covering the constructs
/// `ingest_structure_sql` is expected to model plus the ones it must
/// ledger rather than silently drop.
const STRUCTURE_SQL: &str = r#"--
-- PostgreSQL database dump
--

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);

CREATE SCHEMA widgets_ns;

CREATE EXTENSION IF NOT EXISTS pg_stat_statements WITH SCHEMA public;

CREATE TYPE public.widget_status AS ENUM (
    'draft',
    'published'
);

CREATE FUNCTION public.noop() RETURNS void
    LANGUAGE plpgsql
    AS $$
        BEGIN
          -- a semicolon inside the body must not split this statement;
          SELECT 1;
        END;
        $$;

CREATE TABLE public.companies (
    id bigint NOT NULL,
    name text NOT NULL
);

CREATE TABLE public.widgets (
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    active boolean DEFAULT false,
    status public.widget_status,
    price numeric(10,2) DEFAULT 0,
    published_at timestamp(6) without time zone,
    metadata jsonb,
    external_id uuid,
    schedule tstzrange,
    company_id bigint NOT NULL,
    CONSTRAINT chk_widgets_name_present CHECK ((name <> ''::text))
);

CREATE TABLE public.widgets_2024 (
    id bigint NOT NULL,
    name character varying(255) NOT NULL
);

ALTER TABLE ONLY public.widgets_2024 ATTACH PARTITION public.widgets_2024 FOR VALUES IN ('2024');

CREATE VIEW public.active_widgets AS
 SELECT widgets.id,
    widgets.name
   FROM public.widgets
  WHERE (widgets.active = true);

CREATE UNIQUE INDEX index_widgets_on_name ON public.widgets USING btree (name);

CREATE INDEX index_widgets_on_company_id ON public.widgets USING btree (company_id);

CREATE INDEX index_widgets_on_metadata ON public.widgets USING GIN (metadata);

CREATE INDEX index_widgets_on_custom_method ON public.widgets USING "Odd""Method" (company_id);

CREATE INDEX index_widgets_on_lower_name ON public.widgets USING btree (lower(name));

ALTER TABLE ONLY public.companies
    ADD CONSTRAINT companies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.widgets
    ADD CONSTRAINT fk_rails_widgets_company FOREIGN KEY (company_id) REFERENCES public.companies(id) ON DELETE CASCADE;

CREATE TABLE public.schema_migrations (
    version character varying(255) NOT NULL
);

CREATE TABLE public.ar_internal_metadata (
    key character varying NOT NULL,
    value character varying,
    created_at timestamp without time zone NOT NULL,
    updated_at timestamp without time zone NOT NULL
);

INSERT INTO "schema_migrations" (version) VALUES
('20260101000000'),
('20260102000000');
"#;

fn col<'a>(table: &'a roundhouse::schema::Table, name: &str) -> &'a roundhouse::schema::Column {
    table.columns.iter().find(|c| c.name.as_str() == name).unwrap_or_else(|| {
        panic!("no column {name} on {}; have {:?}", table.name.as_str(), table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>())
    })
}

#[test]
fn ingests_tables_columns_indexes_fk_and_pk() {
    survey::activate();
    let schema =
        ingest_structure_sql(STRUCTURE_SQL.as_bytes(), "db/structure.sql").expect("ingest structure.sql");
    let gaps = survey::drain();

    // Rails bookkeeping tables are never modeled, same as schema.rb.
    assert!(!schema.tables.contains_key(&Symbol::from("schema_migrations")));
    assert!(!schema.tables.contains_key(&Symbol::from("ar_internal_metadata")));

    // `widgets_2024` got its own independent CREATE TABLE (this
    // dialect's real partitioning shape — see the module header) but
    // is later joined to `widgets` via ATTACH PARTITION, which
    // retracts it: modeling every shard of a partitioned table as its
    // own duplicate-schema table would be pure noise.
    assert!(!schema.tables.contains_key(&Symbol::from("widgets_2024")));

    let companies = &schema.tables[&Symbol::from("companies")];
    assert!(col(companies, "id").primary_key, "pk set via ALTER TABLE ... ADD CONSTRAINT");

    let widgets = &schema.tables[&Symbol::from("widgets")];
    assert!(col(widgets, "id").primary_key);
    assert_eq!(col(widgets, "name").col_type, ColumnType::String { limit: Some(255) });
    assert!(!col(widgets, "name").nullable);
    assert_eq!(col(widgets, "description").col_type, ColumnType::Text);
    assert!(col(widgets, "description").nullable);
    assert_eq!(col(widgets, "active").col_type, ColumnType::Boolean);
    // `status` is an enum-typed column — schema.rb parity maps it to String.
    assert_eq!(col(widgets, "status").col_type, ColumnType::String { limit: None });
    assert_eq!(col(widgets, "price").col_type, ColumnType::Decimal { precision: None, scale: None });
    assert_eq!(col(widgets, "published_at").col_type, ColumnType::DateTime);
    assert_eq!(col(widgets, "metadata").col_type, ColumnType::Jsonb);
    assert_eq!(col(widgets, "external_id").col_type, ColumnType::Uuid);
    assert_eq!(col(widgets, "company_id").col_type, ColumnType::BigInt);
    assert!(!col(widgets, "company_id").nullable);

    // `schedule tstzrange` has no `ColumnType` mapping — dropped, and
    // the drop is ledgered, never silent.
    assert!(
        widgets.columns.iter().all(|c| c.name.as_str() != "schedule"),
        "tstzrange column should have been dropped"
    );

    // Indexes: the plain and unique ones land; the expression index
    // (`lower(name)`) is skipped rather than modeled or ledgered (see
    // `handle_create_index`'s doc comment).
    assert!(widgets.indexes.iter().any(|i| i.name.as_str() == "index_widgets_on_name" && i.unique));
    assert!(widgets.indexes.iter().any(|i| i.name.as_str() == "index_widgets_on_company_id" && !i.unique));
    assert_eq!(
        widgets.indexes.iter().find(|i| i.name.as_str() == "index_widgets_on_name").unwrap().using.as_deref(),
        Some("btree")
    );
    assert_eq!(
        widgets.indexes.iter().find(|i| i.name.as_str() == "index_widgets_on_metadata").unwrap().using.as_deref(),
        Some("gin")
    );
    assert_eq!(
        widgets.indexes.iter().find(|i| i.name.as_str() == "index_widgets_on_custom_method").unwrap().using.as_deref(),
        Some("Odd\"Method")
    );
    assert!(!widgets.indexes.iter().any(|i| i.name.as_str() == "index_widgets_on_lower_name"));

    let postgres_ddl = roundhouse::emit::shared::schema_sql::render_schema_statements_for(
        &schema,
        roundhouse::emit::shared::schema_sql::Dialect::Postgres,
    ).expect("PostgreSQL DDL");
    assert!(postgres_ddl.iter().any(|s| s.contains(
        "CREATE INDEX IF NOT EXISTS \"index_widgets_on_metadata\" ON \"widgets\" USING \"gin\" (\"metadata\")"
    )), "{postgres_ddl:?}");
    assert!(postgres_ddl.iter().any(|s| s.contains(
        "CREATE INDEX IF NOT EXISTS \"index_widgets_on_custom_method\" ON \"widgets\" USING \"Odd\"\"Method\" (\"company_id\")"
    )), "{postgres_ddl:?}");

    // Foreign key.
    let fk = widgets.foreign_keys.first().expect("one foreign key");
    assert_eq!(fk.from_column.as_str(), "company_id");
    assert_eq!(fk.to_table.0.as_str(), "companies");
    assert_eq!(fk.to_column.as_str(), "id");
    assert!(matches!(fk.on_delete, roundhouse::schema::ReferentialAction::Cascade));

    // Ledgered gaps: the dropped tstzrange column and the unmodeled view.
    let messages: Vec<String> = gaps.iter().map(|g| format!("{g}")).collect();
    assert!(
        messages.iter().any(|m| m.contains("column dropped: widgets.schedule") && m.contains("tstzrange")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("view not modeled:") && m.contains("(active_widgets)")),
        "{messages:?}"
    );
}

#[test]
fn strict_mode_aborts_on_the_first_gap() {
    let err = ingest_structure_sql(STRUCTURE_SQL.as_bytes(), "db/structure.sql").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported type") || msg.contains("not modeled"),
        "expected a gap message, got: {msg}"
    );
}

#[test]
fn index_access_methods_allow_comments_at_keyword_boundaries() {
    let sql = r#"
CREATE TABLE widgets (payload jsonb);
CREATE INDEX index_before_using_comment ON widgets/* before USING */ USING gin (payload);
CREATE INDEX index_between_using_comment ON widgets USING/* before method */ gin (payload);
CREATE INDEX index_after_method_comment ON widgets USING gin/* after method */ (payload);
CREATE INDEX index_line_after_method_comment ON widgets USING gin -- after method
    (payload);
CREATE INDEX index_quoted_method_boundary ON widgets USING"gin"(payload);
"#;
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("commented index DDL");
    let indexes = &schema.tables[&Symbol::from("widgets")].indexes;
    for name in [
        "index_before_using_comment",
        "index_between_using_comment",
        "index_after_method_comment",
        "index_line_after_method_comment",
        "index_quoted_method_boundary",
    ] {
        let index = indexes.iter().find(|index| index.name.as_str() == name).unwrap_or_else(|| {
            panic!("missing index {name}: {indexes:?}")
        });
        assert_eq!(index.using.as_deref(), Some("gin"), "{name}");
    }
}

#[test]
fn index_access_method_keyword_requires_a_token_boundary() {
    let sql = b"CREATE TABLE widgets (payload jsonb);\nCREATE INDEX malformed ON widgets USINGgin (payload);";
    let error = ingest_structure_sql(sql, "db/structure.sql").unwrap_err();
    assert!(
        error.to_string().contains("CREATE INDEX clause before the column list is not modeled"),
        "{error}"
    );
}

#[test]
fn composite_primary_keys_are_kept_whole_and_foreign_keys_ledgered_not_narrowed() {
    let sql = r#"
CREATE TABLE public.assignments (
    project_id bigint NOT NULL,
    user_id bigint NOT NULL,
    role text
);

CREATE TABLE public.projects (
    id bigint NOT NULL,
    company_id bigint NOT NULL
);

ALTER TABLE ONLY public.assignments
    ADD CONSTRAINT assignments_pkey PRIMARY KEY (project_id, user_id);

ALTER TABLE ONLY public.assignments
    ADD CONSTRAINT assignments_project_fk FOREIGN KEY (project_id, user_id) REFERENCES public.projects(id, company_id);
"#;
    survey::activate();
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("survey mode never errors");
    let gaps = survey::drain();

    let assignments = &schema.tables[&Symbol::from("assignments")];
    assert!(assignments.columns.iter().all(|c| !c.primary_key), "composite pk never narrowed to one column");
    assert!(assignments.foreign_keys.is_empty(), "composite fk never narrowed to one column pair");

    let messages: Vec<String> = gaps.iter().map(|g| format!("{g}")).collect();
    assert_eq!(assignments.primary_key_columns, [Symbol::from("project_id"), Symbol::from("user_id")]);
    assert!(!messages.iter().any(|m| m.contains("primary key dropped")), "a composite key is kept, not ledgered: {messages:?}");
    assert!(messages.iter().any(|m| m.contains("foreign key dropped") && m.contains("composite")), "{messages:?}");
}

#[test]
fn unrecognized_statement_heads_are_ledgered_once_per_head() {
    let sql = "ALTER TABLE ONLY public.widgets REPLICA IDENTITY FULL;\nALTER TABLE ONLY public.companies REPLICA IDENTITY FULL;\n";
    survey::activate();
    let _ = ingest_structure_sql(sql.as_bytes(), "db/structure.sql");
    let gaps = survey::drain();
    let count = gaps.iter().filter(|g| format!("{g}").contains("ALTER TABLE")).count();
    assert_eq!(count, 1, "capped at one ledger entry per distinct statement head: {gaps:?}");
}

/// End-to-end: an app that ships only `db/structure.sql` (no
/// `schema.rb`) still gets its model attributes typed from it.
#[test]
fn ingest_app_types_model_attributes_from_structure_sql_when_schema_rb_is_absent() {
    let files: &[(&str, &str)] = &[
        ("db/structure.sql", STRUCTURE_SQL),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/models/company.rb", "class Company < ApplicationRecord\nend\n"),
    ];
    survey::activate();
    let app = ingest_app_from_tree(tree(files)).expect("ingest app with only structure.sql");
    let _gaps = survey::drain();

    assert!(app.schema.tables.contains_key(&Symbol::from("widgets")));
    assert!(app.schema.tables.contains_key(&Symbol::from("companies")));

    let widget_model = app.models.iter().find(|m| m.name.0.as_str() == "Widget").unwrap_or_else(|| {
        panic!(
            "no Widget model; have {:?}",
            app.models.iter().map(|m| m.name.0.as_str()).collect::<HashSet<_>>()
        )
    });
    // `name` is `character varying(255) NOT NULL` — typed as a bare
    // `Str` (not nullable), derived straight from `db/structure.sql`
    // with no `db/schema.rb` in the tree at all.
    assert_eq!(widget_model.attributes.fields.get(&Symbol::from("name")), Some(&Ty::Str));
}

/// The survey report's `bucket_key` (see `src/ingest/survey.rs`) groups
/// gaps by the message text before the first `(`. The identifier
/// therefore has to sit INSIDE parens for gaps to bucket by *reason*
/// rather than by table name — otherwise every composite-key/view gap
/// gets its own one-off bucket and the report can't show which gap
/// kinds actually dominate. This pins that shape for all four
/// structure.sql-specific ledger templates.
#[test]
fn composite_key_and_view_gaps_bucket_by_reason_not_by_table() {
    let sql = r#"
CREATE TABLE public.assignments (
    project_id bigint NOT NULL,
    user_id bigint NOT NULL
);

CREATE TABLE public.memberships (
    org_id bigint NOT NULL,
    account_id bigint NOT NULL
);

CREATE TABLE public.orgs (
    id bigint NOT NULL,
    other_id bigint NOT NULL
);

ALTER TABLE ONLY public.assignments
    ADD CONSTRAINT assignments_pkey PRIMARY KEY (project_id, user_id);

ALTER TABLE ONLY public.memberships
    ADD CONSTRAINT memberships_pkey PRIMARY KEY (org_id, account_id);

ALTER TABLE ONLY public.assignments
    ADD CONSTRAINT assignments_fk FOREIGN KEY (project_id, user_id) REFERENCES public.orgs(id, other_id);

ALTER TABLE ONLY public.memberships
    ADD CONSTRAINT memberships_fk FOREIGN KEY (org_id, account_id) REFERENCES public.orgs(id, other_id);

CREATE VIEW public.view_one AS SELECT assignments.project_id FROM public.assignments;
CREATE VIEW public.view_two AS SELECT memberships.org_id FROM public.memberships;
"#;
    survey::activate();
    let _ = ingest_structure_sql(sql.as_bytes(), "db/structure.sql");
    let gaps = survey::drain();

    let keys_for = |needle: &str| -> HashSet<String> {
        gaps.iter()
            .filter(|g| format!("{g}").contains(needle))
            .map(|g| roundhouse::ingest::survey::bucket_key(g))
            .collect()
    };

    // Composite primary keys are kept (`Table.primary_key_columns`), not ledgered.
    assert!(keys_for("primary key dropped").is_empty());

    let fk_keys = keys_for("foreign key dropped");
    assert_eq!(fk_keys.len(), 1, "both composite FKs should bucket together: {fk_keys:?}");

    let view_keys = keys_for("view not modeled");
    assert_eq!(view_keys.len(), 1, "both unmodeled views should bucket together: {view_keys:?}");
}

/// A partition child attached via `ALTER TABLE … ATTACH PARTITION …`
/// (rather than declared `PARTITION OF` up front) must not surface as
/// its own table — see `handle_attach_partition`.
#[test]
fn attached_partition_children_are_not_modeled_as_separate_tables() {
    let sql = r#"
CREATE TABLE public.readings (
    id bigint NOT NULL,
    shard_key bigint NOT NULL
)
PARTITION BY HASH (shard_key);

CREATE TABLE shard.readings_p0 (
    id bigint NOT NULL,
    shard_key bigint NOT NULL
);

-- pg_dump's real order: ATTACH PARTITION comes before the shard's OWN
-- constraints (its pkey is dumped alongside the other per-table
-- constraints, well after every table and every partition attachment).
ALTER TABLE ONLY public.readings ATTACH PARTITION shard.readings_p0 FOR VALUES WITH (modulus 2, remainder 0);

ALTER TABLE ONLY public.readings
    ADD CONSTRAINT readings_pkey PRIMARY KEY (id, shard_key);

ALTER TABLE ONLY shard.readings_p0
    ADD CONSTRAINT readings_p0_pkey PRIMARY KEY (id, shard_key);
"#;
    survey::activate();
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("survey mode never errors");
    let gaps = survey::drain();

    assert!(schema.tables.contains_key(&Symbol::from("readings")), "the parent stays");
    assert!(
        !schema.tables.contains_key(&Symbol::from("readings_p0")),
        "the attached shard is retracted, not modeled as its own table"
    );
    // The parent keeps its composite key; the shard's own later `ADD
    // CONSTRAINT ... PRIMARY KEY` targets a table this Schema no longer
    // has, and is silently skipped.
    assert_eq!(
        schema.tables[&Symbol::from("readings")].primary_key_columns,
        [Symbol::from("id"), Symbol::from("shard_key")]
    );
    assert!(!gaps.iter().any(|g| format!("{g}").contains("primary key dropped")), "{gaps:?}");
}

/// pg_dump writes an index on a partitioned table as `CREATE INDEX …
/// ON ONLY parent`, then one index per partition and an `ALTER INDEX …
/// ATTACH PARTITION` joining each to the parent's. `ONLY` used to be read
/// as the table name, so every such index was ledgered as a clause before
/// the column list. The parent keeps the index; the partitions' indexes
/// stay out with the partitions.
#[test]
fn an_index_on_only_a_partitioned_parent_is_the_parents_index() {
    let sql = r#"
CREATE TABLE public.events (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    status integer
)
PARTITION BY RANGE (created_at);

CREATE TABLE public.events_2026_06 (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    status integer
);

CREATE TABLE public.only_items (
    id bigint NOT NULL
);

ALTER TABLE ONLY public.events ATTACH PARTITION public.events_2026_06 FOR VALUES FROM ('2026-06-01 00:00:00') TO ('2026-07-01 00:00:00');

CREATE UNIQUE INDEX idx_events_tenant_identity ON ONLY public.events USING btree (id, created_at, account_id);

CREATE INDEX index_events_on_failures ON ONLY public.events USING btree (status, created_at DESC) WHERE (status >= 400);

CREATE INDEX index_events_on_account_id ON ONLY public.events (account_id);

CREATE UNIQUE INDEX events_2026_06_id_created_at_account_id_idx ON public.events_2026_06 USING btree (id, created_at, account_id);

CREATE INDEX index_only_items_on_id ON only_items USING btree (id);

ALTER INDEX public.idx_events_tenant_identity ATTACH PARTITION public.events_2026_06_id_created_at_account_id_idx;
"#;
    survey::activate();
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("survey mode never errors");
    let gaps = survey::drain();
    assert!(gaps.is_empty(), "{gaps:?}");

    let indexes: Vec<(&str, bool, Option<&str>, Option<&str>)> = schema.tables[&Symbol::from("events")]
        .indexes
        .iter()
        .map(|i| (i.name.as_str(), i.unique, i.using.as_deref(), i.predicate.as_deref()))
        .collect();
    // `index_events_on_failures` has a `DESC` column, which is skipped
    // like any other non-plain column, `ONLY` or not.
    assert_eq!(
        indexes,
        vec![
            ("idx_events_tenant_identity", true, Some("btree"), None),
            ("index_events_on_account_id", false, None, None),
        ]
    );
    assert!(!schema.tables.contains_key(&Symbol::from("events_2026_06")));
    // A table whose name starts with `only` is not read as `ONLY`.
    assert_eq!(schema.tables[&Symbol::from("only_items")].indexes.len(), 1);
}

/// `CREATE … INDEX … WHERE (…)` keeps its predicate, as `where:` in
/// `schema.rb` does. It used to be dropped, so a partial unique index
/// came out unique over every row. The predicate is the rest of the
/// statement, after any `INCLUDE` or `WITH`.
#[test]
fn a_partial_index_keeps_its_predicate() {
    let sql = r#"
CREATE TABLE public.tokens (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    revoked_at timestamp(6) without time zone
);

CREATE UNIQUE INDEX index_tokens_on_live_user_id ON public.tokens USING btree (user_id) WHERE (revoked_at IS NULL);

CREATE INDEX index_tokens_on_revoked ON public.tokens USING btree (user_id) INCLUDE (revoked_at) WITH (fillfactor='90') WHERE ((revoked_at IS NOT NULL) AND (user_id > 0));

CREATE UNIQUE INDEX index_tokens_on_user_id ON public.tokens USING btree (user_id);
"#;
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("ingest structure.sql");
    let predicates: Vec<(&str, bool, Option<&str>)> = schema.tables[&Symbol::from("tokens")]
        .indexes
        .iter()
        .map(|i| (i.name.as_str(), i.unique, i.predicate.as_deref()))
        .collect();
    assert_eq!(
        predicates,
        vec![
            ("index_tokens_on_live_user_id", true, Some("(revoked_at IS NULL)")),
            (
                "index_tokens_on_revoked",
                false,
                Some("((revoked_at IS NOT NULL) AND (user_id > 0))")
            ),
            ("index_tokens_on_user_id", true, None),
        ]
    );
}

/// pg_dump 18 (and 17.6, 16.10, 15.14, 14.19, 13.22) opens a dump with
/// `\restrict <key>` and closes it with `\unrestrict <key>`. Each is a
/// psql meta-command on a line of its own, with no `;`, so it reached
/// the statement split glued to whatever came next, and strict ingest
/// stopped on `structure.sql statement not modeled: \RESTRICT …`. Rails
/// 7.2.3 and 8.0.3 onward strip both lines from the dump; earlier
/// versions keep them, after the leading comments they do strip and
/// before the `SET search_path` they append.
#[test]
fn pg_dump_restrict_lines_are_not_statements() {
    let sql = r#"\restrict zUyUIswlha2RbZMedZzUe8pO4Wf9iauTiSg4I61hfovPxfQRKvvZhhfhXwcY9FH
CREATE TABLE public.notes (
    id bigint NOT NULL,
    body text
);

ALTER TABLE ONLY public.notes
    ADD CONSTRAINT notes_pkey PRIMARY KEY (id);

--
-- PostgreSQL database dump complete
--

\unrestrict zUyUIswlha2RbZMedZzUe8pO4Wf9iauTiSg4I61hfovPxfQRKvvZhhfhXwcY9FH

SET search_path TO "$user", public;

INSERT INTO "schema_migrations" (version) VALUES
('20261002052542');

"#;
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("strict ingest");
    let notes = &schema.tables[&Symbol::from("notes")];
    assert!(col(notes, "id").primary_key);
    assert_eq!(col(notes, "body").col_type, ColumnType::Text);
}

/// Only those two lines are skipped. Any other meta-command, such as the
/// `\connect` that `pg_dump --create` writes, is still ledgered, and the
/// same text inside a string literal is part of the statement.
#[test]
fn other_meta_commands_and_literals_are_left_alone() {
    let connect = "\\connect app_production\n\nCREATE TABLE public.notes (\n    id bigint NOT NULL\n);\n";
    let err = ingest_structure_sql(connect.as_bytes(), "db/structure.sql").unwrap_err();
    assert!(err.to_string().contains("not modeled: \\CONNECT APP_PRODUCTION"), "{err}");

    let sql = "CREATE TABLE public.notes (\n    id bigint NOT NULL,\n    body text DEFAULT 'first line\n\\restrict kept'::text\n);\n";
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("strict ingest");
    let notes = &schema.tables[&Symbol::from("notes")];
    assert_eq!(col(notes, "body").default.as_deref(), Some("first line\n\\restrict kept"));
}

#[test]
fn named_not_null_constraints_keep_the_column() {
    // PostgreSQL 18 pg_dump names a column's NOT NULL whenever the stored
    // constraint name is not `<table>_<column>_not_null`: renamed tables,
    // partitions, and names truncated at 63 bytes.
    let sql = r#"
CREATE TABLE public.user_identities (
    id uuid NOT NULL,
    first_name text,
    email character varying CONSTRAINT identities_email_not_null NOT NULL,
    created_at timestamp(6) without time zone CONSTRAINT identities_created_at_not_null NOT NULL,
    nickname text DEFAULT 'x'::text CONSTRAINT identities_nickname_not_null NOT NULL,
    handle text CONSTRAINT "Identities handle not null" NOT NULL,
    display_name text GENERATED ALWAYS AS (first_name) STORED CONSTRAINT identities_display_name_not_null NOT NULL
);
"#;
    let schema = ingest_structure_sql(sql.as_bytes(), "db/structure.sql").expect("strict ingest");
    let table = &schema.tables[&Symbol::from("user_identities")];
    for name in ["email", "created_at", "nickname", "handle", "display_name"] {
        assert!(!col(table, name).nullable, "{name} keeps NOT NULL");
    }
    assert!(col(table, "first_name").nullable);
    assert!(matches!(col(table, "email").col_type, ColumnType::String { .. }));
    assert!(matches!(col(table, "created_at").col_type, ColumnType::DateTime));
    assert_eq!(col(table, "nickname").default.as_deref(), Some("x"));
    assert_eq!(col(table, "display_name").generated.as_ref().expect("generated").expression, "first_name");
}

#[test]
fn a_named_column_constraint_other_than_nullability_is_refused() {
    // Only the nullability clause is modeled after `CONSTRAINT name`. A
    // named key, check, uniqueness or reference must fail ingest rather
    // than be read as a plain nullable column.
    for (clause, quoted) in [
        ("CONSTRAINT people_id_pkey PRIMARY KEY", "CONSTRAINT people_id_pkey PRIMARY KEY"),
        ("CONSTRAINT people_id_key UNIQUE", "CONSTRAINT people_id_key UNIQUE"),
        ("CONSTRAINT people_id_check CHECK (id > 0)", "CONSTRAINT people_id_check CHECK (id > 0)"),
        ("CONSTRAINT people_id_fkey REFERENCES public.users(id)", "CONSTRAINT people_id_fkey REFERENCES public.users(id)"),
        ("NOT NULL CONSTRAINT people_id_pkey PRIMARY KEY", "CONSTRAINT people_id_pkey PRIMARY KEY"),
    ] {
        let sql = format!("CREATE TABLE public.people (\n    id bigint {clause}\n);\n");
        let error = ingest_structure_sql(sql.as_bytes(), "db/structure.sql")
            .expect_err(clause)
            .to_string();
        assert!(
            error.contains(&format!("column dropped: people.id has unsupported column constraint `{quoted}`")),
            "{clause}: {error}"
        );
    }
}

#[test]
fn a_constraint_keyword_needs_a_name() {
    // `NOT` is a reserved word, so it cannot be the constraint's name:
    // `CONSTRAINT NOT NULL` is malformed, not a named NOT NULL.
    let quoted = "CREATE TABLE public.people (\n    email text CONSTRAINT \"not\" NOT NULL\n);\n";
    let schema = ingest_structure_sql(quoted.as_bytes(), "db/structure.sql").expect("a quoted reserved word is a name");
    assert!(!col(&schema.tables[&Symbol::from("people")], "email").nullable);

    let plain = "CREATE TABLE public.people (\n    email text CONSTRAINT NOT NULL\n);\n";
    let error = ingest_structure_sql(plain.as_bytes(), "db/structure.sql")
        .expect_err("unnamed CONSTRAINT")
        .to_string();
    assert!(error.contains("column dropped: people.email has unsupported column constraint `CONSTRAINT NOT NULL`"), "{error}");

    let generated = r#"
CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    display_name text GENERATED ALWAYS AS (first_name) STORED CONSTRAINT NOT NULL
);
"#;
    let error = ingest_structure_sql(generated.as_bytes(), "db/structure.sql")
        .expect_err("unnamed CONSTRAINT after GENERATED")
        .to_string();
    assert!(error.contains("people.display_name"), "{error}");
}

#[test]
fn a_constraint_keyword_without_a_not_null_still_fails_after_generated() {
    let sql = r#"
CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    display_name text GENERATED ALWAYS AS (first_name) STORED CONSTRAINT people_display_name_check
);
"#;
    let error = ingest_structure_sql(sql.as_bytes(), "db/structure.sql")
        .expect_err("a dangling constraint name is not a nullability clause")
        .to_string();
    assert!(error.contains("generated column dropped: people.display_name"), "{error}");
}
