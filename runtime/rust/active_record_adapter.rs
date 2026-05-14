//! Abstract `ActiveRecordAdapter` trait — the rust analog of
//! crystal's `abstract class ActiveRecordAdapter` and TS's
//! `interface ActiveRecordAdapter`. Hand-written for Phase 3.
//!
//! The 9-method contract `runtime/ruby/active_record/base.rb` calls
//! against `ActiveRecord.adapter`. Every concrete adapter (production
//! sqlite, in-memory framework-test, future libsql/D1) implements it.
//!
//! Return shapes are `serde_json::Value` because the abstract slot is
//! polymorphic — concrete adapters produce concrete row types
//! (`HashMap<String, rusqlite::Value>` for sqlite, an in-memory
//! `TestRow` for the framework-test adapter), and the only common
//! surface is the untyped JSON tree. The transpiled `Base` methods
//! that call into the adapter feed the result through
//! `instantiate(row)` which subclasses override with concrete-typed
//! per-column extraction.

use serde_json::Value;
use std::collections::HashMap;

// Row shape: `HashMap<String, Value>` (not `Value`). The transpiled
// `runtime/ruby/active_record/base.rb` types rows as
// `Hash[String, untyped]` and feeds them to `Self::instantiate(row)`
// whose RBS signature is `(Hash[String, untyped]) -> Base`. Returning
// a bare `Value` would force the transpile to insert an `as_object`-
// style coercion at every call site — handing the trait the
// pre-shaped HashMap keeps the body-typer's view aligned with what
// the runtime delivers.
//
// Parameters use owned `String` (not `&str`) so transpiled call sites
// can pass the result of `Self::table_name() -> String` directly.
// Rust idiom would prefer `&str`, but emit-side auto-borrow at every
// call site is a much wider change than the one-time alloc cost here.
pub type Row = HashMap<String, Value>;

pub trait ActiveRecordAdapter: Send + Sync {
    fn all(&self, table_name: String) -> Vec<Row>;
    fn find(&self, table_name: String, id: i64) -> Option<Row>;
    fn r#where(&self, table_name: String, conditions: HashMap<String, Value>) -> Vec<Row>;
    fn count(&self, table_name: String) -> i64;
    fn exists(&self, table_name: String, id: i64) -> bool;
    fn insert(&self, table_name: String, attributes: HashMap<String, Value>) -> i64;
    fn update(&self, table_name: String, id: i64, attributes: HashMap<String, Value>);
    fn delete(&self, table_name: String, id: i64);
    fn truncate(&self, table_name: String);
}
