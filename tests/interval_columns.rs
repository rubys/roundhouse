//! PostgreSQL `interval` is a schema type (`t.interval`, `t.column …,
//! :interval`, and `interval` in `structure.sql`), not an application
//! special case. Storage is a whole-second count; the public reader is
//! `ActiveSupport::Duration`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::Severity;
use roundhouse::emit::shared::schema_sql::{render_schema_statements_for, Dialect};
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ingest::schema::{ingest_migration, ingest_schema};
use roundhouse::ingest::structure_sql::ingest_structure_sql;
use roundhouse::project::{target_files, BuildTarget};
use roundhouse::schema::ColumnType;
use roundhouse::ty::Ty;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "job_runs", force: :cascade do |t|
    t.interval "duration"
    t.interval "span", precision: 6, null: false
    t.column "elapsed", :interval
  end
end
"#;

fn duration_ty() -> Ty {
    Ty::Class {
        id: roundhouse::ident::ClassId(Symbol::from("ActiveSupport::Duration")),
        args: vec![].into(),
    }
}

#[test]
fn schema_rb_keeps_interval_columns() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").expect("interval is a column type");
    let runs = &schema.tables[&Symbol::from("job_runs")];
    let column = |name: &str| runs.columns.iter().find(|c| c.name.as_str() == name).unwrap();
    assert!(matches!(column("duration").col_type, ColumnType::Interval));
    assert!(column("duration").nullable);
    assert!(matches!(column("span").col_type, ColumnType::Interval));
    assert!(!column("span").nullable);
    assert!(matches!(column("elapsed").col_type, ColumnType::Interval));
    assert_eq!(column("duration").generated_text_compatible, Some(false));

    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join("\n");
    assert!(sqlite.contains("duration INTEGER"), "{sqlite}");
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join("\n");
    assert!(postgres.contains("\"duration\" interval"), "{postgres}");
    assert!(postgres.contains("\"span\" interval NOT NULL"), "{postgres}");
    assert!(!postgres.contains("character varying"), "{postgres}");
}

#[test]
fn a_migration_and_structure_sql_use_the_same_interval_type() {
    let migration = r#"
      class CreateJobRuns < ActiveRecord::Migration[8.1]
        def change
          create_table :job_runs do |t|
            t.interval :duration
          end
        end
      end
    "#;
    let mut schema = roundhouse::schema::Schema::default();
    ingest_migration(migration.as_bytes(), "001_create_job_runs.rb", &mut schema).expect("migration");
    let duration = schema.tables[&Symbol::from("job_runs")]
        .columns
        .iter()
        .find(|c| c.name.as_str() == "duration")
        .expect("migration column");
    assert!(matches!(duration.col_type, ColumnType::Interval));

    let dump = "CREATE TABLE public.job_runs (id bigint NOT NULL, duration interval, span interval(6));";
    let dumped = ingest_structure_sql(dump.as_bytes(), "db/structure.sql").expect("structure.sql");
    let columns = &dumped.tables[&Symbol::from("job_runs")].columns;
    assert!(columns.iter().any(|c| c.name.as_str() == "duration" && matches!(c.col_type, ColumnType::Interval)));
    assert!(columns.iter().any(|c| c.name.as_str() == "span" && matches!(c.col_type, ColumnType::Interval)));
}

#[test]
fn lowered_ruby_reads_an_interval_as_a_duration() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/job_run.rb"),
        b"class JobRun < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        b"class ApplicationController < ActionController::Base\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let row = &app.models.iter().find(|m| m.name.0.as_str() == "JobRun").unwrap().attributes;
    assert_eq!(
        row.fields[&Symbol::from("duration")],
        Ty::Union { variants: vec![duration_ty(), Ty::Nil].into() }
    );
    assert_eq!(row.fields[&Symbol::from("span")], duration_ty());
    let lower = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect();
    assert_eq!(errors, Vec::<String>::new());

    let files = target_files(&app, std::path::Path::new("."), BuildTarget::Ruby).expect("ruby");
    let model = files.iter().find(|(path, _)| path == "app/models/job_run.rb").unwrap();
    assert!(model.1.contains("ActiveSupport.parse_db_interval(@duration_raw)"), "{}", model.1);
    assert!(model.1.contains("ActiveSupport.format_db_interval"), "{}", model.1);
    assert!(model.1.contains("def duration_raw"), "storage stays an integer count:\n{}", model.1);
}

#[test]
fn a_strict_target_refuses_an_interval_column() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/job_run.rb"),
        b"class JobRun < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        b"class ApplicationController < ActionController::Base\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Go, BuildTarget::Roda] {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&app, std::path::Path::new("."), target)
        });
        let errors: Vec<_> = diags
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect();
        assert!(
            errors.iter().any(|message| message.contains("interval")),
            "{target:?} must not emit an interval column quietly:\n{}",
            errors.join("\n")
        );
    }
}
