//! `AdapterInterface` — the concrete type the transpiled
//! `ActiveRecord.adapter` slot uses. Wraps an `Arc<dyn ActiveRecordAdapter>`
//! so the module-singleton emit's slot template
//! (`Mutex<Option<AdapterInterface>>` + `.clone().unwrap_or_default()`)
//! works without per-target rust2 emit branching.
//!
//! Why a wrapper: the `runtime/ruby/active_record/base.rbs` types
//! `ActiveRecord.adapter` as `AdapterInterface` (the analyzer registers
//! that class with the 9-method contract — `all/find/where/count/exists?/
//! insert/update/delete/truncate`). Transpiled call sites
//! (`ActiveRecord::adapter().find(...)`) need a *single* concrete type
//! that:
//!   - Is `Clone` (the slot template does `.clone()` on the mutex guard).
//!   - Has a `Default` (the template falls back to `Default::default()`
//!     when the slot is `None`).
//!   - Forwards every adapter method to whatever concrete impl was
//!     installed at boot (sqlite, framework-test, libsql, ...).
//!
//! A bare `Arc<dyn ActiveRecordAdapter>` lacks `Default`. Wrapping it
//! lets us provide a panicking-on-call "not configured" default
//! (matches the call-time error you'd get if the boot path forgot to
//! install an adapter — earlier than e.g. a SQL error).
//!
//! Install at boot:
//!     ActiveRecord::set_adapter(AdapterInterface::new(SqliteAdapter::open("./db.sqlite")));

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::active_record_adapter::ActiveRecordAdapter;

struct NotConfigured;
impl ActiveRecordAdapter for NotConfigured {
    fn all(&self, _t: &str) -> Vec<Value> {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn find(&self, _t: &str, _id: i64) -> Option<Value> {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn r#where(&self, _t: &str, _c: HashMap<String, Value>) -> Vec<Value> {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn count(&self, _t: &str) -> i64 {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn exists(&self, _t: &str, _id: i64) -> bool {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn insert(&self, _t: &str, _a: HashMap<String, Value>) -> i64 {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn update(&self, _t: &str, _id: i64, _a: HashMap<String, Value>) {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn delete(&self, _t: &str, _id: i64) {
        panic!("ActiveRecord.adapter was not set before use")
    }
    fn truncate(&self, _t: &str) {
        panic!("ActiveRecord.adapter was not set before use")
    }
}

#[derive(Clone)]
pub struct AdapterInterface(Arc<dyn ActiveRecordAdapter + Send + Sync>);

impl Default for AdapterInterface {
    fn default() -> Self {
        Self(Arc::new(NotConfigured))
    }
}

impl AdapterInterface {
    pub fn new<A>(adapter: A) -> Self
    where
        A: ActiveRecordAdapter + Send + Sync + 'static,
    {
        Self(Arc::new(adapter))
    }
}

impl ActiveRecordAdapter for AdapterInterface {
    fn all(&self, t: &str) -> Vec<Value> {
        self.0.all(t)
    }
    fn find(&self, t: &str, id: i64) -> Option<Value> {
        self.0.find(t, id)
    }
    fn r#where(&self, t: &str, c: HashMap<String, Value>) -> Vec<Value> {
        self.0.r#where(t, c)
    }
    fn count(&self, t: &str) -> i64 {
        self.0.count(t)
    }
    fn exists(&self, t: &str, id: i64) -> bool {
        self.0.exists(t, id)
    }
    fn insert(&self, t: &str, a: HashMap<String, Value>) -> i64 {
        self.0.insert(t, a)
    }
    fn update(&self, t: &str, id: i64, a: HashMap<String, Value>) {
        self.0.update(t, id, a)
    }
    fn delete(&self, t: &str, id: i64) {
        self.0.delete(t, id)
    }
    fn truncate(&self, t: &str) {
        self.0.truncate(t)
    }
}
