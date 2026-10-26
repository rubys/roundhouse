//! `Mutex#synchronize`, `Thread::Mutex#synchronize` and
//! `ActiveSupport::Cache::MemoryStore#fetch` answer their block's value,
//! unless the app reopens the class and defines the method itself: then the
//! app's method answers, as Ruby calls it.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

fn binops(files: &[(&str, &str)]) -> Vec<String> {
    let mut tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table \"things\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
    ]
    .iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    for (p, c) in files {
        tree.insert(std::path::PathBuf::from(p), c.as_bytes().to_vec());
    }
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .map(|d| format!("{:?}", d.kind))
        .collect()
}

const CALLS: &str = r#"class Thing < ApplicationRecord
  def self.locked = Mutex.new.synchronize { "x" } + 1
  def self.thread_locked = Thread::Mutex.new.synchronize { "x" } + 1
  def self.cached = ActiveSupport::Cache::MemoryStore.new.fetch("k") { "x" } + 1
  def self.uncached = ActiveSupport::Cache::NullStore.new.fetch("k") { "x" } + 1
  def self.noted = ActiveSupport::Notifications.subscribed(->(event) { event }, "demo") { "x" } + 1
end
"#;

#[test]
fn the_built_in_methods_answer_their_block() {
    let found = binops(&[("app/models/thing.rb", CALLS)]);
    assert_eq!(found.len(), 5, "{found:?}");
}

#[test]
fn an_app_defined_mutex_synchronize_answers_its_own_type() {
    let found = binops(&[
        ("app/models/thing.rb", CALLS),
        ("app/models/mutex_override.rb", "class Mutex\n  def synchronize\n    yield\n    42\n  end\nend\n"),
    ]);
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn an_app_defined_thread_mutex_synchronize_answers_its_own_type() {
    let found = binops(&[
        ("app/models/thing.rb", CALLS),
        ("app/models/thread_mutex_override.rb", "class Thread::Mutex\n  def synchronize\n    yield\n    42\n  end\nend\n"),
    ]);
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn an_app_defined_memory_store_fetch_answers_its_own_type() {
    let found = binops(&[
        ("app/models/thing.rb", CALLS),
        ("app/models/memory_store_override.rb", "class ActiveSupport::Cache::MemoryStore\n  def fetch(name)\n    yield\n    42\n  end\nend\n"),
    ]);
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn an_app_redefinition_that_answers_its_block_keeps_the_block_type() {
    let found = binops(&[
        ("app/models/thing.rb", CALLS),
        ("app/models/mutex_override.rb", "class Mutex\n  def synchronize\n    yield\n  end\nend\n"),
    ]);
    assert_eq!(found.len(), 5, "{found:?}");
}
