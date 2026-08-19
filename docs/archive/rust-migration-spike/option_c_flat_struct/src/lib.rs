//! Option C — flat mega-struct (no traits, no inheritance).
//!
//! Translation strategy: each model becomes a self-contained
//! struct with all inherited fields flattened in + per-model
//! impl block holding everything (validate, save, find, all,
//! attributes serialization). No code reuse across models — each
//! model is its own complete world.
//!
//! This mirrors the current `src/emit/rust/model.rs` emit shape.
//! Useful as the comparison baseline.
//!
//! Cross-model collection types: `Vec<Article>` works.
//! `Vec<Box<dyn ActiveRecord>>` is **impossible** — there's no
//! shared trait to dispatch through.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Nil,
}

/// Adapter contract — same as Option A.
pub trait ActiveRecordAdapter {
    fn find(&self, table: &str, id: i64) -> Option<HashMap<String, CellValue>>;
    fn all(&self, table: &str) -> Vec<HashMap<String, CellValue>>;
    fn count(&self, table: &str) -> i64;
    fn insert(&mut self, table: &str, attrs: &HashMap<&str, CellValue>) -> i64;
    fn update(&mut self, table: &str, id: i64, attrs: &HashMap<&str, CellValue>);
    fn delete(&mut self, table: &str, id: i64);
}

// ─── per-model emit (everything inline; no shared abstractions) ─────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Article {
    // Inherited from Base — flattened in.
    pub id: i64,
    pub errors: Vec<String>,
    pub persisted: bool,
    pub destroyed: bool,
    // Per-model fields.
    pub title: String,
    pub body: String,
}

impl Article {
    // Per-model class methods (no shared trait — duplicated across models).
    pub fn table_name() -> &'static str { "articles" }
    pub fn schema_columns() -> Vec<&'static str> { vec!["id", "title", "body"] }

    pub fn instantiate(row: &HashMap<String, CellValue>) -> Self {
        let mut a = Article::default();
        if let Some(CellValue::Int(v)) = row.get("id") { a.id = *v; }
        if let Some(CellValue::Str(v)) = row.get("title") { a.title = v.clone(); }
        if let Some(CellValue::Str(v)) = row.get("body") { a.body = v.clone(); }
        a
    }

    // Inherited Base behavior — manually copied into every model.
    pub fn id(&self) -> i64 { self.id }
    pub fn persisted(&self) -> bool { self.persisted }
    pub fn destroyed(&self) -> bool { self.destroyed }
    pub fn new_record(&self) -> bool { !self.persisted }
    pub fn errors(&self) -> &Vec<String> { &self.errors }

    // Validations — per-model inlined; no shared helpers.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.title.is_empty() {
            errors.push("title can't be blank".into());
        }
        if self.body.is_empty() {
            errors.push("body can't be blank".into());
        }
        if self.body.len() < 10 {
            errors.push("body is too short (minimum is 10 characters)".into());
        }
        errors
    }

    // Save — per-model duplication of the "validate, then insert/update" pattern.
    pub fn save(&mut self, adapter: &mut dyn ActiveRecordAdapter) -> bool {
        let errs = self.validate();
        if !errs.is_empty() {
            self.errors = errs;
            return false;
        }
        self.errors.clear();
        let mut attrs = HashMap::new();
        attrs.insert("title", CellValue::Str(self.title.clone()));
        attrs.insert("body", CellValue::Str(self.body.clone()));
        if self.id == 0 {
            let id = adapter.insert(Self::table_name(), &attrs);
            self.id = id;
        } else {
            adapter.update(Self::table_name(), self.id, &attrs);
        }
        self.persisted = true;
        true
    }

    pub fn find(adapter: &dyn ActiveRecordAdapter, id: i64) -> Self {
        let row = adapter.find(Self::table_name(), id);
        match row {
            Some(r) => {
                let mut inst = Self::instantiate(&r);
                inst.persisted = true;
                inst
            }
            None => panic!("RecordNotFound: articles id={id}"),
        }
    }

    pub fn all(adapter: &dyn ActiveRecordAdapter) -> Vec<Self> {
        adapter.all(Self::table_name()).iter().map(|row| {
            let mut inst = Self::instantiate(row);
            inst.persisted = true;
            inst
        }).collect()
    }
}

// ─── tiny in-memory adapter (same as Option A) ──────────────────

#[derive(Default)]
pub struct InMemoryAdapter {
    tables: HashMap<String, HashMap<i64, HashMap<String, CellValue>>>,
    next_ids: HashMap<String, i64>,
}

impl ActiveRecordAdapter for InMemoryAdapter {
    fn find(&self, table: &str, id: i64) -> Option<HashMap<String, CellValue>> {
        self.tables.get(table).and_then(|t| t.get(&id)).cloned()
    }
    fn all(&self, table: &str) -> Vec<HashMap<String, CellValue>> {
        self.tables.get(table).map(|t| t.values().cloned().collect()).unwrap_or_default()
    }
    fn count(&self, table: &str) -> i64 {
        self.tables.get(table).map(|t| t.len() as i64).unwrap_or(0)
    }
    fn insert(&mut self, table: &str, attrs: &HashMap<&str, CellValue>) -> i64 {
        let next = self.next_ids.entry(table.to_string()).or_insert(0);
        *next += 1;
        let id = *next;
        let mut row: HashMap<String, CellValue> = attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        row.insert("id".to_string(), CellValue::Int(id));
        self.tables.entry(table.to_string()).or_default().insert(id, row);
        id
    }
    fn update(&mut self, table: &str, id: i64, attrs: &HashMap<&str, CellValue>) {
        if let Some(t) = self.tables.get_mut(table) {
            if let Some(row) = t.get_mut(&id) {
                for (k, v) in attrs { row.insert(k.to_string(), v.clone()); }
            }
        }
    }
    fn delete(&mut self, table: &str, id: i64) {
        if let Some(t) = self.tables.get_mut(table) { t.remove(&id); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_assigns_id_and_persists() {
        let mut adapter = InMemoryAdapter::default();
        let mut a = Article { title: "Hello".into(), body: "Long enough body".into(), ..Default::default() };
        assert!(a.save(&mut adapter));
        assert!(a.persisted());
        assert_ne!(a.id(), 0);
    }

    #[test]
    fn validation_failure_blocks_save() {
        let mut adapter = InMemoryAdapter::default();
        let mut a = Article { title: "".into(), body: "short".into(), ..Default::default() };
        assert!(!a.save(&mut adapter));
        assert_eq!(a.errors().len(), 2);
    }

    #[test]
    fn find_returns_typed_subclass() {
        let mut adapter = InMemoryAdapter::default();
        let mut a = Article { title: "T".into(), body: "Long enough body".into(), ..Default::default() };
        a.save(&mut adapter);
        let id = a.id();
        let found: Article = Article::find(&adapter, id);
        assert_eq!(found.title, "T");
    }

    #[test]
    fn collection_of_concrete_type() {
        let adapter = InMemoryAdapter::default();
        let _v: Vec<Article> = Article::all(&adapter);
    }

    // No `Vec<Box<dyn ActiveRecord>>` test — Option C makes
    // heterogeneous collections impossible by design.
}
