//! A unique index whose `where:` SQLite can't be trusted to run as
//! written — a Postgres dump's `((kind)::text = 'initial'::text)` — is
//! unique over every row in the SQLite DDL, as it was before ingest kept
//! predicates. That index rejects rows Rails accepts, so the transpile
//! names it in a warning (`project::report_sqlite_index_predicates`). A
//! predicate both engines read alike is rendered, and nothing is
//! reported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::analyze::{Analyzer, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "monitor_scans", force: :cascade do |t|
    t.bigint "monitor_id", null: false
    t.string "kind", null: false
    t.datetime "retired_at"
    t.index ["monitor_id"], name: "index_monitor_scans_initial", unique: true, where: "((kind)::text = 'initial'::text)"
    t.index ["monitor_id", "kind"], name: "index_monitor_scans_live", unique: true, where: "(retired_at IS NULL)"
  end
end
"#;

#[test]
fn a_unique_predicate_sqlite_cannot_run_is_named_in_a_warning() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/monitor_scan.rb"),
        b"class MonitorScan < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog");
    let (files, diags) =
        roundhouse::emit::diagnostics::scope(|| target_files(&app, &root, BuildTarget::Ruby));
    let files = files.expect("ruby target files");

    let named: Vec<_> = diags.iter().filter(|d| d.message.contains("partial_unique_index")).collect();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].severity, Severity::Warning);
    assert!(
        named[0].message.contains("index_monitor_scans_initial")
            && named[0].message.contains("((kind)::text = 'initial'::text)"),
        "{}",
        named[0].message
    );

    let schema = files
        .iter()
        .find(|(p, _)| p == "config/schema.rb")
        .map(|(_, c)| c.as_str())
        .expect("config/schema.rb");
    assert!(
        schema.contains("CREATE UNIQUE INDEX IF NOT EXISTS index_monitor_scans_initial ON monitor_scans (monitor_id)\""),
        "{schema}"
    );
    assert!(
        schema.contains(
            "CREATE UNIQUE INDEX IF NOT EXISTS index_monitor_scans_live ON monitor_scans (monitor_id, kind) \
             WHERE (retired_at IS NULL)"
        ),
        "{schema}"
    );
}
