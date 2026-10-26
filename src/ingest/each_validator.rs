//! `validates :slug, presence: true, slug: { reserved: %w[admin new] }`,
//! a key naming an app's `ActiveModel::EachValidator` (`SlugValidator`).
//! Rails 8.1.4 resolves `foo:` with `const_get("FooValidator")` on the
//! model, so `<Model>::FooValidator`, then the same name on each of its
//! superclasses, then `FooValidator` (`"admin/foo":` is
//! `Admin::FooValidator`); a constant in the model's enclosing module is
//! not found. It builds one validator per declaration with
//! `attributes: [...]` and the key's options, and on validation calls
//! `validate_each(record, attribute, value)` per attribute, skipping a nil
//! or blank value under `allow_nil:` / `allow_blank:`, gated by `if:` /
//! `unless:` / `on:`.
//!
//! Ingest keeps such a call's built-in rules and the call itself
//! (`ParsedValidates::Custom`); [`resolve`] then reports the call as
//! before, and drops it whole, when a key names no app class inheriting
//! `ActiveModel::EachValidator` (a gem's validator, say). On the ruby
//! family ([`expand`]) each key becomes model source, as `validates_with`
//! does (`ingest::validates_with`):
//!
//! ```ruby
//! def __validates_each_article_1
//!   __validator = ::SlugValidator.new(attributes: [:slug], reserved: %w[admin new])
//!   __validator.validate_each(self, :slug, self.slug)
//! end
//! validate :__validates_each_article_1, if: -> { … }
//! ```
//!
//! after the call's own checks. The validator is built on each validation
//! rather than once per declaration, which only a validator keeping state
//! between calls would notice, and its `options` hold the key's own
//! options without the conditions and `allow_nil:` / `allow_blank:`.
//! A shape this doesn't take (`strict:`, a non-literal `allow_nil:`, a key
//! whose value is a constant or a call, which Rails classifies by its class)
//! keeps the call the reported gap, as do strict targets ([`refuse`]).

use std::cell::RefCell;

use crate::app::{App, EachValidatorCall};
use crate::dialect::ModelBodyItem;
use crate::ingest::prism_api::PrismLocation;
use crate::span::Span;

thread_local! {
    /// The `validates` calls ingest kept for their custom keys: the call's
    /// span, its source text (a `with_options` call is merged source, not
    /// the file's), and the keys.
    static NOTED: RefCell<Vec<(Span, String, Vec<String>)>> = const { RefCell::new(Vec::new()) };
}

const DETAIL: &str = "validation option not lowered; Rails rejects records this build saves";

pub(super) fn reset() {
    NOTED.with(|n| n.borrow_mut().clear());
}

pub(super) fn note(span: Span, text: String, keys: Vec<String>) {
    NOTED.with(|n| n.borrow_mut().push((span, text, keys)));
}

/// After ingest: each noted call whose every key names an app
/// `EachValidator` is kept for the target ([`expand`] or [`refuse`]);
/// any other is dropped whole and reported, as a gap at ingest is.
pub(super) fn resolve(app: &mut App) {
    let noted = NOTED.with(|n| std::mem::take(&mut *n.borrow_mut()));
    if noted.is_empty() {
        return;
    }
    let mut gaps: Vec<(Span, String)> = Vec::new();
    let mut calls: Vec<EachValidatorCall> = Vec::new();
    for mi in 0..app.models.len() {
        let model = &app.models[mi];
        let mut dropped: Vec<Span> = Vec::new();
        for item in &model.body {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let Some((span, text, keys)) = noted.iter().find(|(span, ..)| *span == expr.span) else { continue };
            let resolved: Vec<(String, Option<String>)> =
                keys.iter().map(|key| (key.clone(), validator_class(app, mi, key))).collect();
            if resolved.iter().all(|(_, class)| class.is_some()) {
                calls.push(EachValidatorCall {
                    model: model.name.clone(),
                    span: *span,
                    text: text.clone(),
                    keys: resolved.into_iter().map(|(key, class)| (key, class.unwrap_or_default())).collect(),
                });
            } else {
                let unresolved: Vec<String> =
                    resolved.into_iter().filter(|(_, class)| class.is_none()).map(|(key, _)| key).collect();
                gaps.extend(constructs(text, &unresolved).into_iter().map(|c| (*span, c)));
                dropped.push(*span);
            }
        }
        drop_calls(&mut app.models[mi], &dropped);
    }
    app.each_validator_calls = calls;
    super::app::report_validates_gaps(app, gaps, DETAIL);
}

/// The ruby family: each kept call becomes its generated methods and
/// `validate` declarations, in place. A call it can't generate is
/// dropped and reported for `target`.
pub(crate) fn expand(app: &mut App, target: &str) -> Vec<crate::diagnostic::Diagnostic> {
    let calls = std::mem::take(&mut app.each_validator_calls);
    let mut gaps: Vec<(Span, String)> = Vec::new();
    // One count for the app, as `validates_with` keeps.
    let mut counter = 0usize;
    for model in &mut app.models {
        let owner = crate::naming::underscore_words(model.name.0.as_str());
        let mut body = Vec::with_capacity(model.body.len());
        let mut dropped: Vec<Span> = Vec::new();
        for item in std::mem::take(&mut model.body) {
            let call = match &item {
                ModelBodyItem::Unknown { expr, .. } => {
                    calls.iter().find(|c| c.model == model.name && c.span == expr.span)
                }
                _ => None,
            };
            let Some(call) = call else {
                body.push(item);
                continue;
            };
            let expanded = generated_source(call, &owner, &mut counter).and_then(|src| {
                if src.is_empty() {
                    Some(Vec::new())
                } else {
                    super::validates_with::ingest_source(&src, &model.name)
                }
            });
            match expanded {
                Some(items) => body.extend(items),
                None => {
                    let keys: Vec<String> = call.keys.iter().map(|(key, _)| key.clone()).collect();
                    gaps.extend(constructs(&call.text, &keys).into_iter().map(|c| (call.span, c)));
                    dropped.push(call.span);
                }
            }
        }
        model.body = body;
        drop_calls(model, &dropped);
    }
    super::app::validates_gap_diagnostics(app, target, gaps, DETAIL)
}

/// A target without the expansion: every kept call is dropped and
/// reported for `target`, with the built-in rules beside its keys.
pub(crate) fn refuse(app: &mut App, target: &str) -> Vec<crate::diagnostic::Diagnostic> {
    let calls = std::mem::take(&mut app.each_validator_calls);
    let mut gaps: Vec<(Span, String)> = Vec::new();
    for model in &mut app.models {
        let mine: Vec<&EachValidatorCall> = calls.iter().filter(|c| c.model == model.name).collect();
        for call in &mine {
            let keys: Vec<String> = call.keys.iter().map(|(key, _)| key.clone()).collect();
            gaps.extend(constructs(&call.text, &keys).into_iter().map(|c| (call.span, c)));
        }
        let spans: Vec<Span> = mine.iter().map(|c| c.span).collect();
        drop_calls(model, &spans);
    }
    super::app::validates_gap_diagnostics(app, target, gaps, DETAIL)
}

/// Every item a dropped call produced: its rules' `Validation` items and
/// the call itself, which share its span.
fn drop_calls(model: &mut crate::dialect::Model, spans: &[Span]) {
    if spans.is_empty() {
        return;
    }
    model.body.retain(|item| {
        !matches!(item, ModelBodyItem::Validation { .. } | ModelBodyItem::Unknown { .. }) || !spans.contains(&item.span())
    });
}

/// `validates :a, key:` per attribute and key, as an ingest gap names it.
fn constructs(text: &str, keys: &[String]) -> Vec<String> {
    let attrs = shape(text).map(|s| s.attrs).unwrap_or_default();
    if attrs.is_empty() {
        return keys.iter().map(|key| format!("validates {key}:")).collect();
    }
    attrs.iter().flat_map(|attr| keys.iter().map(move |key| format!("validates :{attr}, {key}:"))).collect()
}

/// The app class `key` names for the model at `mi`, Rails' lookup order:
/// the model's own namespace, each superclass's, then the top level.
fn validator_class(app: &App, mi: usize, key: &str) -> Option<String> {
    let name = format!("{}Validator", crate::naming::camelize_path(&key.replace("::", "/")));
    let mut scopes: Vec<String> = Vec::new();
    let mut current = Some(app.models[mi].name.0.as_str().to_string());
    while let Some(scope) = current {
        if scopes.contains(&scope) {
            break;
        }
        current = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == scope)
            .and_then(|m| m.parent.as_ref())
            .map(|p| p.0.as_str().to_string());
        scopes.push(scope);
    }
    scopes
        .iter()
        .map(|scope| format!("{scope}::{name}"))
        .chain(std::iter::once(name.clone()))
        .find(|candidate| each_validator(app, candidate))
}

/// An app class inheriting `ActiveModel::EachValidator`, directly or
/// through other app classes.
fn each_validator(app: &App, name: &str) -> bool {
    let mut current = name.to_string();
    for _ in 0..app.library_classes.len() + 1 {
        let Some(class) = app.library_classes.iter().find(|c| !c.is_module && c.name.0.as_str() == current) else {
            return false;
        };
        match class.parent.as_ref().map(|p| p.0.as_str().trim_start_matches("::")) {
            Some("ActiveModel::EachValidator") => return true,
            Some(parent) => current = parent.to_string(),
            None => return false,
        }
    }
    false
}

/// A `validates` call as the expansion reads it.
struct Shape {
    attrs: Vec<String>,
    /// `if:` / `unless:` / `on:`, by name, with the value's text.
    conditions: Vec<(String, String)>,
    allow_nil: bool,
    allow_blank: bool,
    /// Each key's value node text and kind.
    keys: Vec<(String, KeyValue)>,
}

enum KeyValue {
    /// `true`: no options.
    Bare,
    /// `false` / `nil`: Rails adds no validator.
    Off,
    /// A Hash: its options, conditions and `allow_*`, as the call's.
    Hash { options: Vec<String>, conditions: Vec<(String, String)>, allow_nil: Option<bool>, allow_blank: Option<bool> },
    /// Any other value: `in:` for an Array or a Range, else `with:`.
    Value(String),
    /// A constant or a call: Rails picks by its class at run time (a Hash
    /// is the options, an Array `in:`), which isn't known here.
    Dynamic,
}

fn shape(text: &str) -> Option<Shape> {
    let result = ruby_prism::parse(text.as_bytes());
    let root = result.node();
    let call = root.as_program_node()?.statements().body().iter().next()?.as_call_node()?;
    if call.receiver().is_some() || call.block().is_some() {
        return None;
    }
    let slice = |loc: ruby_prism::Location<'_>| -> Option<String> {
        text.get(loc.start_offset()..loc.end_offset()).map(str::to_string)
    };
    let mut shape = Shape { attrs: Vec::new(), conditions: Vec::new(), allow_nil: false, allow_blank: false, keys: Vec::new() };
    let mut hash = None;
    for arg in call.arguments()?.arguments().iter() {
        if let Some(name) = super::util::symbol_or_string_value(&arg) {
            shape.attrs.push(name);
        } else if let Some(options) = arg.as_keyword_hash_node() {
            hash = Some(options);
        } else {
            return None;
        }
    }
    for el in hash?.elements().iter() {
        let assoc = el.as_assoc_node()?;
        let key = String::from_utf8_lossy(assoc.key().as_symbol_node()?.unescaped()).into_owned();
        let value = assoc.value();
        match key.as_str() {
            "if" | "unless" | "on" => shape.conditions.push((key, slice(value.location())?)),
            "allow_nil" => shape.allow_nil = literal_bool(&value)?,
            "allow_blank" => shape.allow_blank = literal_bool(&value)?,
            "strict" => return None,
            _ => {
                let kind = if value.as_true_node().is_some() {
                    KeyValue::Bare
                } else if value.as_false_node().is_some() || value.as_nil_node().is_some() {
                    KeyValue::Off
                } else if let Some(h) = value.as_hash_node() {
                    let mut options = Vec::new();
                    let mut conditions = Vec::new();
                    let (mut allow_nil, mut allow_blank) = (None, None);
                    for el in h.elements().iter() {
                        let assoc = el.as_assoc_node()?;
                        let name = String::from_utf8_lossy(assoc.key().as_symbol_node()?.unescaped()).into_owned();
                        let value = assoc.value();
                        match name.as_str() {
                            "if" | "unless" | "on" => conditions.push((name, slice(value.location())?)),
                            "allow_nil" => allow_nil = Some(literal_bool(&value)?),
                            "allow_blank" => allow_blank = Some(literal_bool(&value)?),
                            "strict" => return None,
                            _ => options.push(slice(assoc.location())?),
                        }
                    }
                    KeyValue::Hash { options, conditions, allow_nil, allow_blank }
                } else if value.as_array_node().is_some() || value.as_range_node().is_some() {
                    // Rails' `_parse_validates_options`: a Range or an Array is `in:`.
                    KeyValue::Value(format!("in: {}", slice(value.location())?))
                } else if literal_with(&value) {
                    KeyValue::Value(format!("with: {}", slice(value.location())?))
                } else {
                    KeyValue::Dynamic
                };
                shape.keys.push((key, kind));
            }
        }
    }
    Some(shape)
}

/// A literal that is neither a Hash, an Array nor a Range: Rails passes it
/// as `with:`.
fn literal_with(node: &ruby_prism::Node<'_>) -> bool {
    node.as_string_node().is_some()
        || node.as_interpolated_string_node().is_some()
        || node.as_symbol_node().is_some()
        || node.as_integer_node().is_some()
        || node.as_float_node().is_some()
        || node.as_regular_expression_node().is_some()
        || node.as_interpolated_regular_expression_node().is_some()
        || node.as_lambda_node().is_some()
}

fn literal_bool(node: &ruby_prism::Node<'_>) -> Option<bool> {
    if node.as_true_node().is_some() {
        Some(true)
    } else if node.as_false_node().is_some() {
        Some(false)
    } else {
        None
    }
}

/// A reader the generated method can call as `self.<attr>`.
fn plain_reader(attr: &str) -> bool {
    let mut chars = attr.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The model source for one call's custom keys, one method and one
/// `validate` per key in the call's order (empty when every key is off),
/// or None for a shape it doesn't take.
fn generated_source(call: &EachValidatorCall, owner: &str, counter: &mut usize) -> Option<String> {
    let shape = shape(&call.text)?;
    if shape.attrs.is_empty() || !shape.attrs.iter().all(|a| plain_reader(a)) {
        return None;
    }
    let attrs = shape.attrs.iter().map(|a| format!(":{a}")).collect::<Vec<_>>().join(", ");
    let mut out = String::from("class GeneratedValidatesEach\n");
    let first = *counter;
    for (key, class) in &call.keys {
        let (_, value) = shape.keys.iter().find(|(k, _)| k == key)?;
        let mut conditions = shape.conditions.clone();
        let (mut allow_nil, mut allow_blank) = (shape.allow_nil, shape.allow_blank);
        let options: Vec<String> = match value {
            KeyValue::Off => continue,
            KeyValue::Dynamic => return None,
            KeyValue::Bare => Vec::new(),
            KeyValue::Value(option) => vec![option.clone()],
            KeyValue::Hash { options, conditions: own, allow_nil: nil, allow_blank: blank } => {
                // The key's own conditions and `allow_*` win over the call's.
                for (name, text) in own {
                    conditions.retain(|(n, _)| n != name);
                    conditions.push((name.clone(), text.clone()));
                }
                allow_nil = nil.unwrap_or(allow_nil);
                allow_blank = blank.unwrap_or(allow_blank);
                options.clone()
            }
        };
        *counter += 1;
        let name = format!("__validates_each_{owner}_{counter}");
        let built = std::iter::once(format!("attributes: [{attrs}]")).chain(options).collect::<Vec<_>>().join(", ");
        // Root-qualified: the class is the one Rails' lookup found, and a
        // bare name inside a model nested in a module would resolve
        // lexically, to that module's class of the name first.
        out.push_str(&format!("  def {name}\n    __validator = ::{class}.new({built})\n"));
        for attr in &shape.attrs {
            let guard = if allow_blank {
                format!(" unless ActiveSupport.blank?(self.{attr})")
            } else if allow_nil {
                format!(" unless self.{attr}.nil?")
            } else {
                String::new()
            };
            out.push_str(&format!("    __validator.validate_each(self, :{attr}, self.{attr}){guard}\n"));
        }
        out.push_str("  end\n\n");
        let validate = std::iter::once(format!(":{name}"))
            .chain(conditions.iter().map(|(n, text)| format!("{n}: {text}")))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  validate {validate}\n"));
    }
    if *counter == first {
        return Some(String::new());
    }
    out.push_str("end\n");
    Some(out)
}
