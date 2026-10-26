//! A record as a jbuilder VALUE: `json.owner @gadget.widget`,
//! `json.extract!(@gadget, :widget)`, `json.(@widget, :gadgets)`.
//!
//! Jbuilder stores the value as it is and the template's JSON encoding
//! calls `as_json` on it: Active Record's `serializable_hash`, the
//! record's attributes under String keys (its table's columns, an enum
//! under its label, without the inheritance column), and a collection
//! of records is an array of those. The runtime's
//! `JsonBuilder.encode_value` has no `as_json` to call on every target,
//! so it wrote the record's `to_s` (`"#<Widget:0x…>"`).
//!
//! The lowering knows the record's model from its static type, and the
//! model's attributes from its table, so it writes the object down as
//! `json.extract!(record, *attributes)` would, `null` for a nil record.
//! A record of a closed set of models (a union of model types, a
//! polymorphic reader whose targets the IR records) is written by a
//! switch on its class, each branch its model's attributes, and raises
//! for a class outside the set. A record whose attributes are not the
//! table's columns (the attribute API, `serialize`, a renamed
//! inheritance column, its own `as_json`) or whose class is not known
//! (a polymorphic reader with no recorded targets) is refused with the
//! reason rather than written as its `to_s`.

use std::collections::HashMap;

use crate::app::App;
use crate::dialect::Association;
use crate::expr::Expr;
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// A value that is records of one model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RecordValue {
    /// One record, or nil.
    One(String),
    /// An Array, Relation or association collection of records.
    Many(String),
    /// One record of any of a closed set of models, or nil.
    OneOf(Vec<String>),
}

/// What the lowering knows about the app's models as JSON values.
#[derive(Default)]
pub(super) struct RecordShapes {
    /// Each model's `serializable_hash` keys, or why they are not known.
    attributes: HashMap<String, Result<Vec<Symbol>, &'static str>>,
    /// Each model's association readers and the records they answer;
    /// `None` for a polymorphic `belongs_to` with no recorded targets.
    readers: HashMap<String, HashMap<Symbol, Option<RecordValue>>>,
}

impl RecordShapes {
    pub(super) fn build(app: &App) -> Self {
        let mut out = RecordShapes::default();
        for m in &app.models {
            let name = m.name.0.as_str().to_string();
            let own_json = m
                .methods()
                .any(|d| matches!(d.name.as_str(), "as_json" | "serializable_hash" | "attributes"));
            let attrs = if own_json {
                Err("the model defines its own `as_json`, `serializable_hash` or `attributes`")
            } else {
                match super::attribute_names(&name, app) {
                    // Active Record's `serializable_hash` excepts the
                    // inheritance column.
                    Some(cols) => Ok(cols.into_iter().filter(|c| c != "type").map(Symbol::from).collect()),
                    None => Err("the model's attributes are not its table's columns"),
                }
            };
            out.attributes.insert(name, attrs);
        }
        for m in &app.models {
            let name = m.name.0.as_str().to_string();
            let mut readers = HashMap::new();
            for a in m.associations() {
                let (reader, value) = match a {
                    Association::BelongsTo { name, polymorphic: true, polymorphic_targets, .. } => {
                        let targets: Vec<String> = polymorphic_targets.iter().map(|t| t.0.as_str().to_string()).collect();
                        let closed = !targets.is_empty() && targets.iter().all(|t| out.is_model(t));
                        (name, closed.then_some(RecordValue::OneOf(targets)))
                    }
                    Association::BelongsTo { name, target, .. } | Association::HasOne { name, target, .. } => {
                        (name, Some(RecordValue::One(target.0.as_str().to_string())))
                    }
                    Association::HasMany { name, target, .. } | Association::HasAndBelongsToMany { name, target, .. } => {
                        (name, Some(RecordValue::Many(target.0.as_str().to_string())))
                    }
                };
                readers.insert(reader.clone(), value);
            }
            out.readers.insert(name, readers);
        }
        out
    }

    fn is_model(&self, name: &str) -> bool {
        self.attributes.contains_key(name)
    }

    /// The records a value of type `ty` is, when it is records of one
    /// model.
    pub(super) fn of_ty(&self, ty: &Ty) -> Option<RecordValue> {
        let model = |t: &Ty| match t {
            Ty::Class { id, .. } if self.is_model(id.0.as_str()) => Some(id.0.as_str().to_string()),
            _ => None,
        };
        match ty {
            Ty::Class { id, args } => {
                if self.is_model(id.0.as_str()) {
                    return Some(RecordValue::One(id.0.as_str().to_string()));
                }
                let collection = id.0.as_str().ends_with("Relation") || id.0.as_str().ends_with("CollectionProxy");
                match args.as_slice() {
                    [elem] if collection => model(elem).map(RecordValue::Many),
                    _ => None,
                }
            }
            Ty::Array { elem } => model(elem).map(RecordValue::Many),
            Ty::Relation { of } if self.is_model(of.0.as_str()) => Some(RecordValue::Many(of.0.as_str().to_string())),
            Ty::Union { variants } => {
                let non_nil: Vec<&Ty> = variants.iter().filter(|t| !matches!(t, Ty::Nil)).collect();
                match non_nil.as_slice() {
                    [one] => match self.of_ty(one)? {
                        one @ (RecordValue::One(_) | RecordValue::OneOf(_)) => Some(one),
                        // A collection or nil: Jbuilder writes `null`
                        // for the nil; not a shape lowered here.
                        RecordValue::Many(_) => None,
                    },
                    many => {
                        let mut set: Vec<String> = Vec::new();
                        for t in many {
                            let m = model(t)?;
                            if !set.contains(&m) {
                                set.push(m);
                            }
                        }
                        Some(RecordValue::OneOf(set))
                    }
                }
            }
            _ => None,
        }
    }

    /// The records `model#reader` answers: `Some(None)` for a reader
    /// whose class is decided by the row (polymorphic), `None` when the
    /// reader is no association.
    pub(super) fn reader(&self, model: &str, reader: &Symbol) -> Option<Option<RecordValue>> {
        self.readers.get(model)?.get(reader).cloned()
    }

    pub(super) fn attributes(&self, model: &str) -> Result<&[Symbol], &'static str> {
        match self.attributes.get(model) {
            Some(Ok(a)) => Ok(a),
            Some(Err(why)) => Err(why),
            None => Err("not a model of the app"),
        }
    }
}

/// The analyzer's type of each expression of a template as written,
/// by span, the outermost expression first when two share one (`a&.b`
/// is `a && a.b` over the same source).
pub(super) fn span_types(body: &Expr) -> HashMap<Span, Ty> {
    let mut out = HashMap::new();
    collect(body, &mut out);
    out
}

fn collect(e: &Expr, out: &mut HashMap<Span, Ty>) {
    if let Some(ty) = &e.ty {
        if !e.span.is_synthetic() {
            out.entry(e.span).or_insert_with(|| ty.clone());
        }
    }
    e.node.for_each_child(&mut |c| collect(c, out));
}
