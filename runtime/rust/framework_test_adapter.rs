//! Test-only adapter — rust analog of crystal's `FrameworkTestAdapter`
//! and TS's `FrameworkTestAdapter` singleton. Mirrors the in-memory
//! storage from `runtime/ruby/test/test_helper.rb`'s pure-Ruby module
//! version. Phase 3 hand-written primitive runtime.
//!
//! Framework tests under `runtime/ruby/test/` use it as both the
//! abstract adapter slot (`ActiveRecord.adapter = adapter`) and as a
//! direct receiver for the test-helper API (`create_table`,
//! `drop_table`, `reset_all!`, `schema`).
//!
//! Two surface decisions matching the typed-targets contract:
//!   1. `create_table` takes columns + foreign_keys (the
//!      framework-runtime schema.rb DDL helper shape).
//!   2. `insert` honors an explicit `id` in attrs (framework tests
//!      pre-assign ids: `insert("stubs", id: 7)`); the production
//!      sqlite adapter always autogenerates via `last_insert_rowid()`.

use crate::active_record_adapter::ActiveRecordAdapter;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct TestSchema {
    pub columns: Vec<String>,
    pub foreign_keys: Vec<String>,
}

pub struct FrameworkTestAdapter {
    inner: Mutex<TestState>,
}

struct TestState {
    tables: HashMap<String, HashMap<i64, HashMap<String, Value>>>,
    next_ids: HashMap<String, i64>,
    schemas: HashMap<String, TestSchema>,
}

impl FrameworkTestAdapter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TestState {
                tables: HashMap::new(),
                next_ids: HashMap::new(),
                schemas: HashMap::new(),
            }),
        }
    }

    pub fn reset_all(&self) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        s.tables.clear();
        s.next_ids.clear();
        s.schemas.clear();
    }

    pub fn create_table(&self, name: &str, columns: Vec<String>, foreign_keys: Vec<String>) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        s.tables.insert(name.to_string(), HashMap::new());
        s.next_ids.insert(name.to_string(), 0);
        s.schemas.insert(
            name.to_string(),
            TestSchema {
                columns,
                foreign_keys,
            },
        );
    }

    pub fn drop_table(&self, name: &str) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        s.tables.remove(name);
        s.next_ids.remove(name);
        s.schemas.remove(name);
    }
}

impl Default for FrameworkTestAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveRecordAdapter for FrameworkTestAdapter {
    fn all(&self, table_name: String) -> Vec<Value> {
        let s = self.inner.lock().expect("framework test adapter lock");
        match s.tables.get(&table_name) {
            Some(t) => t
                .values()
                .map(|row| Value::Object(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
                .collect(),
            None => Vec::new(),
        }
    }

    fn find(&self, table_name: String, id: i64) -> Option<Value> {
        let s = self.inner.lock().expect("framework test adapter lock");
        s.tables
            .get(&table_name)
            .and_then(|t| t.get(&id))
            .map(|row| Value::Object(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
    }

    fn r#where(&self, table_name: String, conditions: HashMap<String, Value>) -> Vec<Value> {
        let s = self.inner.lock().expect("framework test adapter lock");
        let Some(t) = s.tables.get(&table_name) else {
            return Vec::new();
        };
        t.values()
            .filter(|row| conditions.iter().all(|(k, v)| row.get(k) == Some(v)))
            .map(|row| Value::Object(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
            .collect()
    }

    fn count(&self, table_name: String) -> i64 {
        let s = self.inner.lock().expect("framework test adapter lock");
        s.tables.get(&table_name).map_or(0, |t| t.len() as i64)
    }

    fn exists(&self, table_name: String, id: i64) -> bool {
        let s = self.inner.lock().expect("framework test adapter lock");
        s.tables
            .get(&table_name)
            .map_or(false, |t| t.contains_key(&id))
    }

    fn insert(&self, table_name: String, attributes: HashMap<String, Value>) -> i64 {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        if !s.tables.contains_key(&table_name) {
            panic!("table {table_name} not created");
        }
        let explicit = attributes.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let current = *s.next_ids.get(&table_name).unwrap_or(&0);
        let id = if explicit != 0 { explicit } else { current + 1 };
        s.next_ids.insert(table_name.clone(), current.max(id));
        let mut row = attributes;
        row.insert("id".to_string(), Value::from(id));
        s.tables.get_mut(&table_name).unwrap().insert(id, row);
        id
    }

    fn update(&self, table_name: String, id: i64, attributes: HashMap<String, Value>) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        let Some(t) = s.tables.get_mut(&table_name) else {
            return;
        };
        let Some(existing) = t.get_mut(&id) else {
            return;
        };
        for (k, v) in attributes {
            existing.insert(k, v);
        }
        existing.insert("id".to_string(), Value::from(id));
    }

    fn delete(&self, table_name: String, id: i64) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        if let Some(t) = s.tables.get_mut(&table_name) {
            t.remove(&id);
        }
    }

    fn truncate(&self, table_name: String) {
        let mut s = self.inner.lock().expect("framework test adapter lock");
        s.tables.insert(table_name.clone(), HashMap::new());
        s.next_ids.insert(table_name, 0);
    }
}
