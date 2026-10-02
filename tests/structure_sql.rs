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
    assert_eq!(col(widgets, "metadata").col_type, ColumnType::Json);
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
    assert!(!widgets.indexes.iter().any(|i| i.name.as_str() == "index_widgets_on_lower_name"));

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
fn composite_primary_and_foreign_keys_are_ledgered_not_narrowed() {
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
    assert!(messages.iter().any(|m| m.contains("primary key dropped") && m.contains("composite")), "{messages:?}");
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

    let pk_keys = keys_for("primary key dropped");
    assert_eq!(pk_keys.len(), 1, "both composite PKs should bucket together: {pk_keys:?}");

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
    // Exactly one composite-pk gap (the parent's), not two — the
    // shard's own later `ADD CONSTRAINT ... PRIMARY KEY` targets a
    // table this Schema no longer has, and is silently skipped rather
    // than ledgering a gap for a table nobody will ever see.
    let pk_gaps = gaps.iter().filter(|g| format!("{g}").contains("primary key dropped")).count();
    assert_eq!(pk_gaps, 1, "{gaps:?}");
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
