//! Target-neutral fixture-loading lowering.
//!
//! Resolves every YAML fixture record into a structured plan:
//! which class it becomes, which columns receive literals, and which
//! columns are foreign-key references to another fixture's eventual
//! AUTOINCREMENT rowid. Per-target emitters consume the plan to
//! render `_load_all()` bodies + labeled getters in their own syntax.
//!
//! The lowering does NOT embed any persistence or runtime surface —
//! it only describes *what* goes into each INSERT, not *how* the
//! target wraps it. The Rust emitter wraps it in `article.save()`
//! plus a thread-local id map; Python might use sqlite3 directly;
//! Crystal might emit DB.exec. That's per-target.

use crate::dialect::{Association, Fixture, Model};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use crate::App;

/// Every fixture in an app, in declaration order. Emitters render
/// this once as a flat loader that runs at test setup; cross-fixture
/// FK references resolve through runtime lookup keyed on
/// `(target_fixture, target_label)`.
#[derive(Clone, Debug, PartialEq)]
pub struct LoweredFixtureSet {
    pub fixtures: Vec<LoweredFixture>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoweredFixture {
    /// The YAML filename stem: `articles.yml` → `"articles"`.
    pub name: Symbol,
    /// The model class these records hydrate into: `Article`.
    pub class: ClassId,
    /// Records in declaration order.
    pub records: Vec<LoweredFixtureRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoweredFixtureRecord {
    pub label: Symbol,
    pub fields: Vec<LoweredFixtureField>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoweredFixtureField {
    pub column: Symbol,
    pub value: LoweredFixtureValue,
}

/// How to source a column's value at test-setup time.
#[derive(Clone, Debug, PartialEq)]
pub enum LoweredFixtureValue {
    /// Literal scalar from the YAML, typed by the column's schema. The
    /// `raw` field is the YAML string form; emitters apply
    /// target-specific literal syntax (Rust `"foo".to_string()`,
    /// Python `"foo"`, etc.).
    Literal { ty: Ty, raw: String },
    /// `article: one` in `comments.yml` — a reference to another
    /// fixture's label. Resolves to that fixture's AUTOINCREMENT id
    /// at runtime, because id assignment only happens after the
    /// referenced record INSERTs.
    FkLookup {
        target_fixture: Symbol,
        target_label: Symbol,
    },
}

pub fn lower_fixtures(app: &App) -> LoweredFixtureSet {
    let fixtures = app
        .fixtures
        .iter()
        .map(|f| lower_fixture(f, app))
        .collect();
    LoweredFixtureSet { fixtures }
}

fn lower_fixture(fixture: &Fixture, app: &App) -> LoweredFixture {
    let class_name = crate::naming::singularize_camelize(fixture.name.as_str());
    let class = ClassId(Symbol::from(class_name.as_str()));
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == class_name.as_str());

    let records = fixture
        .records
        .iter()
        .map(|(label, raw_fields)| LoweredFixtureRecord {
            label: label.clone(),
            fields: raw_fields
                .iter()
                .filter_map(|(k, v)| resolve_field(k, v, model, app))
                .collect(),
        })
        .collect();

    LoweredFixture {
        name: fixture.name.clone(),
        class,
        records,
    }
}

/// Resolve one raw (key, value) entry into a lowered field. Returns
/// `None` when the key doesn't match a known column or association —
/// caller silently drops such entries today, mirroring railcar's
/// tolerance for scaffolding-only columns.
fn resolve_field(
    key: &Symbol,
    value: &str,
    model: Option<&Model>,
    app: &App,
) -> Option<LoweredFixtureField> {
    let model = model?;

    if let Some(ty) = model.attributes.fields.get(key) {
        return Some(LoweredFixtureField {
            column: key.clone(),
            value: LoweredFixtureValue::Literal {
                ty: ty.clone(),
                raw: value.to_string(),
            },
        });
    }

    for assoc in model.associations() {
        if let Association::BelongsTo {
            name,
            target,
            foreign_key,
            ..
        } = assoc
        {
            if name == key {
                let target_fixture = Symbol::from(
                    crate::naming::pluralize_snake(target.0.as_str()).as_str(),
                );
                let target_label = Symbol::from(value);
                let referenced = app
                    .fixtures
                    .iter()
                    .find(|f| f.name.as_str() == target_fixture.as_str());
                if referenced
                    .map(|f| f.records.keys().any(|l| l.as_str() == value))
                    .unwrap_or(false)
                {
                    return Some(LoweredFixtureField {
                        column: foreign_key.clone(),
                        value: LoweredFixtureValue::FkLookup {
                            target_fixture,
                            target_label,
                        },
                    });
                }
            }
        }
    }
    None
}
