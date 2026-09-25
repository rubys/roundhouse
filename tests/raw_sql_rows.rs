//! Raw-SQL rows through `ActiveRecord::Base.connection`.
//!
//! `connection` used to type as `Untyped`, so nothing downstream of a
//! raw query was known — lobsters' FlaggedCommenters reads
//!
//! ```ruby
//! ActiveRecord::Base.connection.exec_query(sql).first.symbolize_keys!
//! ```
//!
//! and the `symbolize_keys!` could not be grounded (no Hash has it at
//! runtime). The connection surface now comes from the runtime's own
//! `connection.rbs`, the row is `Hash[String, untyped]?`, and the
//! String-keyed conversion routes to `ActiveSupport.symbolize_keys`.
//! The bang form on a NAMED receiver stays put: the new hash cannot
//! stand in for a mutation someone reads back.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n".into()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into()),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n".into()),
        ("app/models/stats.rb", format!("class Stats\n  def aggregates\n{body}\n  end\nend\n")),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_raw_row_symbolizes_through_the_runtime() {
    let out = emitted("    ActiveRecord::Base.connection.exec_query(\"select 1 as n\").first.symbolize_keys!");
    assert!(
        out.contains("ActiveSupport.symbolize_keys(ActiveRecord::Base.connection.exec_query(\"select 1 as n\").first)"),
        "{out}"
    );
}

#[test]
fn the_bang_form_on_a_local_is_left_alone() {
    let out = emitted("    row = ActiveRecord::Base.connection.exec_query(\"select 1 as n\").first\n    row.symbolize_keys!\n    row");
    assert!(out.contains("row.symbolize_keys!"), "{out}");
}
