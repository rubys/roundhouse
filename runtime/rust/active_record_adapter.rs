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

pub trait ActiveRecordAdapter: Send + Sync {
    fn all(&self, table_name: &str) -> Vec<Value>;
    fn find(&self, table_name: &str, id: i64) -> Option<Value>;
    fn r#where(&self, table_name: &str, conditions: HashMap<String, Value>) -> Vec<Value>;
    fn count(&self, table_name: &str) -> i64;
    fn exists(&self, table_name: &str, id: i64) -> bool;
    fn insert(&self, table_name: &str, attributes: HashMap<String, Value>) -> i64;
    fn update(&self, table_name: &str, id: i64, attributes: HashMap<String, Value>);
    fn delete(&self, table_name: &str, id: i64);
    fn truncate(&self, table_name: &str);
}
