//! Option A — trait + default methods + struct composition.
//!
//! Translation strategy for `class Article < ActiveRecord::Base; include Validations`:
//! * `BaseFields` — plain struct embedding the inherited fields
//!   (id, errors, persisted, destroyed). Each model embeds it as
//!   a `base: BaseFields` field.
//! * `ActiveRecord` trait with `base()`/`base_mut()` accessors.
//!   Default methods (`id()`, `persisted()`, `save()`, etc.) call
//!   through the accessors. Per-model overrides only when needed.
//! * `Validations` trait with default `validates_*_of` methods.
//!   Models override `validate()` only.
//! * `CellValue` enum — heterogeneous attribute values (the
//!   `attributes()` Hash<Symbol, untyped> in the framework Ruby).
//!
//! Cross-model collection types: `Vec<Article>` works (concrete
//! type). `Vec<Box<dyn ActiveRecord>>` works only after splitting
//! `ActiveRecord` into a `dyn`-compatible subset (no `Self`-typed
//! methods, no associated types) — see `ActiveRecordObject` below.

use std::collections::HashMap;

// ─── runtime/active_record/base.rs ──────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BaseFields {
    pub id: i64,
    pub errors: Vec<String>,
    pub persisted: bool,
    pub destroyed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Nil,
}

/// 9-method adapter contract (mirrors crystal's abstract class).
pub trait ActiveRecordAdapter {
    fn find(&self, table: &str, id: i64) -> Option<HashMap<String, CellValue>>;
    fn all(&self, table: &str) -> Vec<HashMap<String, CellValue>>;
    fn where_(&self, table: &str, conditions: &HashMap<&str, CellValue>) -> Vec<HashMap<String, CellValue>>;
    fn count(&self, table: &str) -> i64;
    fn exists(&self, table: &str, id: i64) -> bool;
    fn insert(&mut self, table: &str, attrs: &HashMap<&str, CellValue>) -> i64;
    fn update(&mut self, table: &str, id: i64, attrs: &HashMap<&str, CellValue>);
    fn delete(&mut self, table: &str, id: i64);
    fn truncate(&mut self, table: &str);
}

/// Sized-bound trait — the per-class methods that can't appear on
/// `dyn ActiveRecord`. `find`, `all`, etc. return `Self`, so they
/// require Sized.
pub trait ActiveRecord: Sized {
    // Per-impl required (the lowerer-emitted methods):
    fn table_name() -> &'static str;
    fn schema_columns() -> Vec<&'static str>;
    fn instantiate(row: &HashMap<String, CellValue>) -> Self;

    // Per-instance accessors to inherited fields.
    fn base(&self) -> &BaseFields;
    fn base_mut(&mut self) -> &mut BaseFields;

    // Per-model attribute serialization (lowerer-emitted from
    // attr_accessor list).
    fn attributes(&self) -> HashMap<&'static str, CellValue>;

    // ── default methods (the inherited Base behavior) ──────────

    fn id(&self) -> i64 { self.base().id }
    fn errors(&self) -> &Vec<String> { &self.base().errors }
    fn persisted(&self) -> bool { self.base().persisted }
    fn destroyed(&self) -> bool { self.base().destroyed }
    fn new_record(&self) -> bool { !self.base().persisted }
    fn mark_persisted(&mut self) { self.base_mut().persisted = true }

    fn find(adapter: &dyn ActiveRecordAdapter, id: i64) -> Self {
        let row = adapter.find(Self::table_name(), id);
        match row {
            Some(r) => {
                let mut inst = Self::instantiate(&r);
                inst.mark_persisted();
                inst
            }
            None => panic!("RecordNotFound: {} id={}", Self::table_name(), id),
        }
    }

    fn all(adapter: &dyn ActiveRecordAdapter) -> Vec<Self> {
        adapter
            .all(Self::table_name())
            .iter()
            .map(|row| {
                let mut inst = Self::instantiate(row);
                inst.mark_persisted();
                inst
            })
            .collect()
    }

    fn count(adapter: &dyn ActiveRecordAdapter) -> i64 {
        adapter.count(Self::table_name())
    }

    fn save(&mut self, adapter: &mut dyn ActiveRecordAdapter) -> bool
    where
        Self: Validations,
    {
        let errs = self.collect_errors();
        if !errs.is_empty() {
            self.base_mut().errors = errs;
            return false;
        }
        self.base_mut().errors.clear();
        let attrs = self.attributes();
        if self.base().id == 0 {
            let id = adapter.insert(Self::table_name(), &attrs);
            self.base_mut().id = id;
        } else {
            adapter.update(Self::table_name(), self.base().id, &attrs);
        }
        self.base_mut().persisted = true;
        true
    }
}

/// dyn-compatible subset for heterogeneous collections
/// (`Vec<Box<dyn ActiveRecordObject>>`). Methods deliberately named
/// `obj_*` to avoid colliding with `ActiveRecord`'s methods of the
/// same role — the blanket `impl<T: ActiveRecord>` would otherwise
/// produce ambiguous-method-resolution errors at call sites that
/// have both traits in scope. Each `ActiveRecord` impl gets the
/// blanket forwarder for free.
pub trait ActiveRecordObject {
    fn obj_id(&self) -> i64;
    fn obj_persisted(&self) -> bool;
    fn obj_destroyed(&self) -> bool;
    fn obj_errors(&self) -> &Vec<String>;
}

impl<T: ActiveRecord> ActiveRecordObject for T {
    fn obj_id(&self) -> i64 { ActiveRecord::id(self) }
    fn obj_persisted(&self) -> bool { ActiveRecord::persisted(self) }
    fn obj_destroyed(&self) -> bool { ActiveRecord::destroyed(self) }
    fn obj_errors(&self) -> &Vec<String> { ActiveRecord::errors(self) }
}

// ─── runtime/active_record/validations.rs ───────────────────────

pub trait Validations: ActiveRecord {
    /// Per-model override; default no-op.
    fn validate(&self, errors: &mut Vec<String>) {
        let _ = errors;
    }

    fn collect_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        self.validate(&mut errors);
        errors
    }

    fn validates_presence_of(attr_name: &str, value: &str, errors: &mut Vec<String>) {
        if value.is_empty() {
            errors.push(format!("{attr_name} can't be blank"));
        }
    }

    fn validates_length_of(
        attr_name: &str,
        value: &str,
        min: Option<usize>,
        max: Option<usize>,
        errors: &mut Vec<String>,
    ) {
        if let Some(m) = min {
            if value.len() < m {
                errors.push(format!(
                    "{attr_name} is too short (minimum is {m} characters)"
                ));
            }
        }
        if let Some(m) = max {
            if value.len() > m {
                errors.push(format!(
                    "{attr_name} is too long (maximum is {m} characters)"
                ));
            }
        }
    }
}

// ─── per-model emit (this is what the lowerer would produce) ────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Article {
    pub base: BaseFields,
    pub title: String,
    pub body: String,
}

impl ActiveRecord for Article {
    fn table_name() -> &'static str { "articles" }
    fn schema_columns() -> Vec<&'static str> { vec!["id", "title", "body"] }

    fn instantiate(row: &HashMap<String, CellValue>) -> Self {
        let mut a = Article::default();
        if let Some(CellValue::Int(v)) = row.get("id") { a.base.id = *v; }
        if let Some(CellValue::Str(v)) = row.get("title") { a.title = v.clone(); }
        if let Some(CellValue::Str(v)) = row.get("body") { a.body = v.clone(); }
        a
    }

    fn base(&self) -> &BaseFields { &self.base }
    fn base_mut(&mut self) -> &mut BaseFields { &mut self.base }

    fn attributes(&self) -> HashMap<&'static str, CellValue> {
        let mut h = HashMap::new();
        h.insert("title", CellValue::Str(self.title.clone()));
        h.insert("body", CellValue::Str(self.body.clone()));
        h
    }
}

impl Validations for Article {
    fn validate(&self, errors: &mut Vec<String>) {
        Self::validates_presence_of("title", &self.title, errors);
        Self::validates_presence_of("body", &self.body, errors);
        Self::validates_length_of("body", &self.body, Some(10), None, errors);
    }
}

// ─── tiny in-memory FrameworkTestAdapter (for the spike's tests) ──

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
    fn where_(&self, table: &str, conditions: &HashMap<&str, CellValue>) -> Vec<HashMap<String, CellValue>> {
        self.all(table).into_iter().filter(|row| {
            conditions.iter().all(|(k, v)| row.get(*k) == Some(v))
        }).collect()
    }
    fn count(&self, table: &str) -> i64 {
        self.tables.get(table).map(|t| t.len() as i64).unwrap_or(0)
    }
    fn exists(&self, table: &str, id: i64) -> bool {
        self.find(table, id).is_some()
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
    fn truncate(&mut self, table: &str) {
        self.tables.remove(table); self.next_ids.remove(table);
    }
}

// ─── tests demonstrating each concern ───────────────────────────

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
        assert!(!a.persisted());
        // title blank + body too short = 2 errors. body presence
        // passes because "short" isn't empty.
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

    #[test]
    fn collection_of_dyn_works_via_object_subtrait() {
        let mut adapter = InMemoryAdapter::default();
        let mut a = Article { title: "T".into(), body: "Long enough body".into(), ..Default::default() };
        a.save(&mut adapter);
        // Heterogeneous collection — possible via the dyn-compatible subset trait.
        let v: Vec<Box<dyn ActiveRecordObject>> = vec![Box::new(a)];
        assert_eq!(v[0].obj_id(), 1);
    }
}
