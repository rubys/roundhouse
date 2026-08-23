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

use crate::dialect::{Association, Fixture, FixtureValue, Model};
use crate::expr::Expr;
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
    /// Statements from the file's `<% … %>` ERB tags, to run once
    /// ahead of the inserts and in the same scope as the values.
    /// Empty for every fixture without ERB.
    pub preamble: Vec<Expr>,
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
    /// A `<%= … %>` tag's Ruby, as an expression. Unlike `Literal`
    /// there is no `raw` to re-render per target: the value only
    /// exists once the expression runs, so a target can carry it only
    /// if it can execute Ruby-shaped IR. Ruby-family emitters splice
    /// the expression into the loader body; the others skip the field
    /// and say so in the generated source.
    Ruby(Expr),
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
    // From the PATH, not the flattened name: a fixture in a
    // subdirectory is namespaced (`push/subscriptions` ->
    // `Push::Subscription`), and `singularize_camelize` over the
    // flattened `push_subscriptions` names `PushSubscription` — a class
    // no model registers, so `model` below comes back `None` and every
    // field is dropped as "not a known column".
    let class_name = crate::naming::classify_path(fixture.path.as_str());
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
                .flat_map(|(k, v)| resolve_field(k, v, model, app))
                .collect(),
        })
        .collect();

    LoweredFixture {
        name: fixture.name.clone(),
        class,
        records,
        preamble: fixture.preamble.clone(),
    }
}

/// Resolve one raw (key, value) entry into the lowered field(s) it
/// names. EMPTY when the key doesn't match a known column or
/// association — caller silently drops such entries today, mirroring
/// railcar's tolerance for scaffolding-only columns.
///
/// A Vec rather than an Option because one key can name TWO columns: a
/// POLYMORPHIC reference (`record: first (Message)`) writes both the id
/// and the type, and writing only one of them is worse than writing
/// neither — a row keyed to the right id under no type belongs to
/// every model at once.
fn resolve_field(
    key: &Symbol,
    value: &FixtureValue,
    model: Option<&Model>,
    app: &App,
) -> Vec<LoweredFixtureField> {
    resolve_field_inner(key, value, model, app).unwrap_or_default()
}

fn resolve_field_inner(
    key: &Symbol,
    value: &FixtureValue,
    model: Option<&Model>,
    app: &App,
) -> Option<Vec<LoweredFixtureField>> {
    let model = model?;

    if let Some(ty) = model.attributes.fields.get(key) {
        return Some(vec![LoweredFixtureField {
            column: key.clone(),
            value: match value {
                FixtureValue::Scalar(raw) => {
                    // An `enum` column's fixture value is the LABEL —
                    // campfire's `users.yml` says `role: administrator`
                    // where the column stores `1`. Rails maps it when it
                    // loads the fixture; the mapping is on the model, so
                    // map it here rather than writing the label text into
                    // an integer column. Without this David loaded with
                    // `role = "administrator"` and `david.administrator?`
                    // — which compares against `1` — answered false, so
                    // every test that signs in as an admin failed its
                    // first assertion.
                    let (ty, raw) = enum_stored_value(model, key, raw)
                        .unwrap_or_else(|| (ty.clone(), raw.clone()));
                    LoweredFixtureValue::Literal { ty, raw }
                }
                FixtureValue::Ruby(expr) => LoweredFixtureValue::Ruby(expr.clone()),
            },
        }]);
    }

    // Only a scalar can name another fixture's label — `creator: <%=
    // … %>` would be an id-producing expression, and that lands on the
    // column branch above (or nowhere) rather than here.
    let Some(value) = value.as_scalar() else {
        return None;
    };
    // A label may be written as a YAML SYMBOL — campfire's rooms.yml
    // says `creator: :david` where its messages.yml says `creator:
    // david`. Both name the same fixture; Rails resolves the reference
    // by name and a Symbol scalar stringifies to it. Strip the sigil
    // for the lookup ONLY — the column branch above keeps its raw
    // scalar, so a genuine String column whose value starts with `:`
    // is untouched.
    //
    // Silently dropping the field is what makes this expensive: `Room`
    // validates its `creator` present, the generated loader calls
    // `save` and not `save!`, so all seven rooms vanished without a
    // word — and every messages row referencing one went with them.
    let value = value.strip_prefix(':').unwrap_or(value);

    for assoc in model.associations() {
        if let Association::BelongsTo {
            name,
            target,
            foreign_key,
            polymorphic,
            ..
        } = assoc
        {
            // A POLYMORPHIC reference names its class inline, which is
            // how Rails' fixtures spell the two columns as one key:
            //
            //   record: first (Message)   ->  record_id, record_type
            //
            // The declared `target` is meaningless here (the synthesizer
            // parks a placeholder), so the class comes from the value.
            if *polymorphic && name == key {
                let Some((label, class)) = split_polymorphic_reference(value) else {
                    continue;
                };
                let target_fixture = Symbol::from(
                    crate::naming::pluralize_snake(class.as_str()).as_str(),
                );
                let known = app
                    .fixtures
                    .iter()
                    .find(|f| f.name.as_str() == target_fixture.as_str())
                    .map(|f| f.records.keys().any(|l| l.as_str() == label))
                    .unwrap_or(false);
                if !known {
                    continue;
                }
                return Some(vec![
                    LoweredFixtureField {
                        column: foreign_key.clone(),
                        value: LoweredFixtureValue::FkLookup {
                            target_fixture,
                            target_label: Symbol::from(label.as_str()),
                        },
                    },
                    LoweredFixtureField {
                        column: Symbol::from(format!("{}_type", name.as_str())),
                        value: LoweredFixtureValue::Literal {
                            ty: Ty::Str,
                            raw: class,
                        },
                    },
                ]);
            }
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
                    return Some(vec![LoweredFixtureField {
                        column: foreign_key.clone(),
                        value: LoweredFixtureValue::FkLookup {
                            target_fixture,
                            target_label,
                        },
                    }]);
                }
            }
        }
    }
    None
}

/// `first (Message)` -> `("first", "Message")`. Rails' own spelling for
/// a polymorphic fixture reference, and the only one it accepts —
/// anything else (a bare label, a Ruby expression) is not a reference
/// this can resolve, because the type column has nowhere to come from.
fn split_polymorphic_reference(scalar: &str) -> Option<(String, String)> {
    let (label, rest) = scalar.split_once(" (")?;
    let class = rest.strip_suffix(')')?;
    if label.is_empty() || class.is_empty() {
        return None;
    }
    Some((label.trim().to_string(), class.trim().to_string()))
}

/// The value an enum column actually stores for `raw`, when `raw` names
/// one of that column's labels. `None` for a non-enum column or a value
/// that is not a label (a column whose enum maps `"active" => "active"`
/// stores the label itself, and this answers that too — harmlessly).
///
/// Reads the same `Model::enums` table `lower::enum_symbols` uses for
/// hand-written `where(role: :bot)`; the two are the query side and the
/// fixture side of one fact.
fn enum_stored_value(
    model: &Model,
    column: &Symbol,
    raw: &str,
) -> Option<(Ty, String)> {
    let labels = model.enums.get(column)?;
    let (_, stored) = labels.iter().find(|(label, _)| label == raw)?;
    match stored {
        crate::expr::Literal::Int { value } => Some((Ty::Int, value.to_string())),
        crate::expr::Literal::Str { value } => Some((Ty::Str, value.clone())),
        _ => None,
    }
}
