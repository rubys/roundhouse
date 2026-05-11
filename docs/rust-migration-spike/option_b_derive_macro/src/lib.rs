//! Option B — macro-driven per-model declaration.
//!
//! Shown here with `macro_rules!` for spike economy; in production
//! this would be a `#[derive(ActiveRecord)]` proc-macro in a sibling
//! `roundhouse-derive` crate. The end-user shape is essentially the
//! same — terse per-model declaration, expansion produces what
//! Option A would write by hand.
//!
//! End-user emit per model: ~10 LOC (struct + macro invocation).
//!
//! Hidden cost: the `active_record!` macro definition itself
//! (~80 LOC here for one validation pattern; a production proc-macro
//! handling the full validates_*_of catalog + custom validate
//! callbacks + nested types would be 500-1500 LOC of macro code).
//!
//! Cross-model collection types: same as Option A — both `Vec<Article>`
//! and (with the right trait split) `Vec<Box<dyn ActiveRecordObject>>`
//! work.

use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BaseFields {
    pub id: i64,
    pub errors: Vec<String>,
    pub persisted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellValue { Str(String), Int(i64), Bool(bool), Nil }

pub trait ActiveRecordAdapter {
    fn insert(&mut self, table: &str, attrs: &HashMap<&str, CellValue>) -> i64;
    fn find(&self, table: &str, id: i64) -> Option<HashMap<String, CellValue>>;
}

pub trait ActiveRecord: Sized {
    fn table_name() -> &'static str;
    fn base(&self) -> &BaseFields;
    fn base_mut(&mut self) -> &mut BaseFields;
    fn attributes(&self) -> HashMap<&'static str, CellValue>;
    fn validate_self(&self, errors: &mut Vec<String>);

    fn id(&self) -> i64 { self.base().id }
    fn persisted(&self) -> bool { self.base().persisted }
    fn errors(&self) -> &Vec<String> { &self.base().errors }

    fn save(&mut self, adapter: &mut dyn ActiveRecordAdapter) -> bool {
        let mut errs = Vec::new();
        self.validate_self(&mut errs);
        if !errs.is_empty() { self.base_mut().errors = errs; return false; }
        let attrs = self.attributes();
        let id = adapter.insert(Self::table_name(), &attrs);
        self.base_mut().id = id;
        self.base_mut().persisted = true;
        true
    }
}

// ─── the macro that hides the boilerplate ───────────────────────
//
// Real version would be `#[derive(ActiveRecord)]` + attribute
// macros (`#[validates(presence)]`, `#[validates(length(min = 10))]`).
// macro_rules! is the spike's stand-in.

macro_rules! active_record {
    (
        $name:ident,
        table = $table:literal,
        fields = { $($fname:ident : $fty:ty),* $(,)? },
        validates = { $($vfield:ident : $vrule:tt),* $(,)? }
    ) => {
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name {
            pub base: BaseFields,
            $(pub $fname : $fty),*
        }

        impl ActiveRecord for $name {
            fn table_name() -> &'static str { $table }
            fn base(&self) -> &BaseFields { &self.base }
            fn base_mut(&mut self) -> &mut BaseFields { &mut self.base }

            fn attributes(&self) -> HashMap<&'static str, CellValue> {
                let mut h = HashMap::new();
                $( h.insert(stringify!($fname), CellValue::Str(self.$fname.to_string())); )*
                h
            }

            fn validate_self(&self, errors: &mut Vec<String>) {
                $( active_record!(@rule self errors $vfield $vrule); )*
            }
        }
    };

    // Per-rule expansion arms.
    (@rule $self_:ident $errors:ident $field:ident presence) => {
        if $self_.$field.is_empty() {
            $errors.push(format!("{} can't be blank", stringify!($field)));
        }
    };
    (@rule $self_:ident $errors:ident $field:ident (length(min = $min:literal))) => {
        if $self_.$field.len() < $min {
            $errors.push(format!(
                "{} is too short (minimum is {} characters)",
                stringify!($field), $min
            ));
        }
    };
}

// ─── per-model emit (this is what the lowerer would produce) ────
//
// Strikingly compact: ~10 LOC per model. Compare to Option A's
// ~42 LOC and Option C's ~95 LOC.

active_record!(
    Article,
    table = "articles",
    fields = { title: String, body: String },
    validates = { title: presence, body: presence, body: (length(min = 10)) }
);

// ─── tiny adapter (same shape as Options A/C) ───────────────────

#[derive(Default)]
pub struct InMemoryAdapter {
    tables: HashMap<String, HashMap<i64, HashMap<String, CellValue>>>,
    next_ids: HashMap<String, i64>,
}

impl ActiveRecordAdapter for InMemoryAdapter {
    fn insert(&mut self, table: &str, attrs: &HashMap<&str, CellValue>) -> i64 {
        let next = self.next_ids.entry(table.to_string()).or_insert(0);
        *next += 1;
        let id = *next;
        let mut row: HashMap<String, CellValue> = attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        row.insert("id".to_string(), CellValue::Int(id));
        self.tables.entry(table.to_string()).or_default().insert(id, row);
        id
    }
    fn find(&self, table: &str, id: i64) -> Option<HashMap<String, CellValue>> {
        self.tables.get(table).and_then(|t| t.get(&id)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_validate() {
        let mut adapter = InMemoryAdapter::default();
        let mut a = Article { title: "Hello".into(), body: "Long enough body".into(), ..Default::default() };
        assert!(a.save(&mut adapter));
        assert_ne!(a.id(), 0);

        let mut bad = Article { title: "".into(), body: "short".into(), ..Default::default() };
        assert!(!bad.save(&mut adapter));
        assert_eq!(bad.errors().len(), 2);
    }

    #[test]
    fn collection_of_concrete_type() {
        let _v: Vec<Article> = Vec::new();
    }
}
