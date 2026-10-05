//! An integer's `limit:` is its size in bytes, and Rails' PostgreSQL and
//! MySQL adapters make 5 to 8 a `bigint`. The shared SQLite DDL stores
//! both as INTEGER and every model types both as `Integer`, so the
//! change shows in the two places that spell the column's width: the
//! Postgres dialect (unit-tested in `emit::shared::schema_sql`) and the
//! Roda conversion's Sequel migrations, pinned here.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn the_roda_migration_declares_an_eight_byte_integer_as_bignum() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "entries", force: :cascade do |t|
    t.integer "key_hash", limit: 8, null: false
    t.integer "byte_size", limit: 4, null: false
  end
end
"#
        .to_vec(),
    );
    tree.insert(PathBuf::from("app/models/entry.rb"), b"class Entry < ApplicationRecord\nend\n".to_vec());
    tree.insert(PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec());
    let app = ingest_app_from_tree(tree).expect("ingest");
    let migration = roundhouse::emit::roda::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("_create_entries.rb"))
        .expect("the entries migration")
        .content;
    assert!(migration.contains("Bignum :key_hash, null: false"), "{migration}");
    assert!(migration.contains("Integer :byte_size, null: false"), "{migration}");
}
