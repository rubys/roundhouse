//! `has_json :settings, restrict_room_creation_to_administrators: false`
//! — ActiveModel::SchematizedJson: a flat JSON object living in one
//! column, whose keys and scalar types the declaration names.
//!
//! Rails returns a `DataAccessor` from the reader and answers every key
//! through `method_missing`. Neither half survives here: the accessor
//! object would need a live back-reference into the owning record to
//! make `account.settings.foo = true` visible in `account[:settings]`,
//! and `method_missing` is off the table
//! ([[feedback_runtime_must_be_statically_resolvable]]). But the schema
//! is a compile-time fact, so the DSL EXPANDS — the metadata-setting
//! half of the DSLs-expand-vs-stay rule — into one typed accessor
//! triple per key, flattened onto the model:
//!
//!   def settings_restrict_room_creation_to_administrators
//!   def settings_restrict_room_creation_to_administrators?
//!   def settings_restrict_room_creation_to_administrators=(value)
//!
//! and the two-hop call sites (`account.settings.foo?`) rewrite to the
//! one-hop flat name. This is the same shape as `lower::typed_store`,
//! whose column is the YAML twin of this one: `@<col>` stays the
//! serialized TEXT that goes to the database, and reads/writes route
//! through ONE named runtime seam — here `SchematizedJson.read_*`/`write_*`
//! (`runtime/spinel/schematized_json.rb` and its CRuby overlay twin —
//! see that file's header for why it is per-target and not transpiled
//! framework Ruby).
//! `has_delegated_json` (Rails' variant that also aliases each key onto
//! the model itself) is not claimed: no corpus app declares one, and it
//! is `has_json` plus aliases, so it can land the day one does.
//!
//! **The declaration is what types the column.** Without it a `json`
//! column is stored text nothing parses (see `ty_of_column`); the
//! per-key accessors are the structure, not a `Hash` the whole column
//! decodes to. That is also why an unsupported schema entry (a
//! symbol-declared type, whose Rails default is nil — a value none of
//! these typed readers can return) makes the whole declaration stay
//! UNCLAIMED and keep warning, rather than being half-expanded.

use std::collections::{HashMap, HashSet};

use crate::dialect::{AccessorKind, MethodDef, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

use super::model_to_library::push_synth_instance_method;

/// The three scalar types `has_json` allows ("Only the three basic JSON
/// types are supported: boolean, integer, and string. No nesting
/// either.").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum JsonScalar {
    Bool,
    Int,
    Str,
}

impl JsonScalar {
    pub(crate) fn ty(self) -> Ty {
        match self {
            JsonScalar::Bool => Ty::Bool,
            JsonScalar::Int => Ty::Int,
            JsonScalar::Str => Ty::Str,
        }
    }

    fn reader(self) -> &'static str {
        match self {
            JsonScalar::Bool => "read_boolean",
            JsonScalar::Int => "read_integer",
            JsonScalar::Str => "read_string",
        }
    }

    fn writer(self) -> &'static str {
        match self {
            JsonScalar::Bool => "write_boolean",
            JsonScalar::Int => "write_integer",
            JsonScalar::Str => "write_string",
        }
    }
}

pub(crate) struct HasJsonAttr {
    pub(crate) name: Symbol,
    pub(crate) scalar: JsonScalar,
    /// The declared default, always a literal — a schema entry without
    /// one leaves the declaration unclaimed (see the module docs).
    pub(crate) default: Literal,
}

pub(crate) struct HasJsonDecl {
    /// The `has_json …` call's own span, so the unlowered-DSL check in
    /// `model_to_library` can ask this parser which declarations it
    /// claimed instead of re-deriving the shape.
    pub(crate) span: Span,
    pub(crate) column: Symbol,
    pub(crate) attrs: Vec<HasJsonAttr>,
}

/// Every `has_json :<col>, <key>: <default>, …` in a model body that
/// this lowering can fully expand. A declaration with any entry it
/// cannot (a symbol-declared type, a non-scalar default, no keys at
/// all) is omitted WHOLE — half of a schema is not a schema.
pub(crate) fn has_json_decls(body: &[ModelBodyItem]) -> Vec<HasJsonDecl> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node
        else {
            continue;
        };
        if method.as_str() != "has_json" {
            continue;
        }
        let Some(column) = args.first().and_then(sym_lit) else { continue };
        let mut attrs = Vec::new();
        let mut complete = true;
        for arg in &args[1..] {
            let ExprNode::Hash { entries, .. } = &*arg.node else {
                complete = false;
                continue;
            };
            for (key, value) in entries {
                let (Some(name), ExprNode::Lit { value: default }) =
                    (sym_lit(key), &*value.node)
                else {
                    complete = false;
                    continue;
                };
                let scalar = match default {
                    Literal::Bool { .. } => JsonScalar::Bool,
                    Literal::Int { .. } => JsonScalar::Int,
                    Literal::Str { .. } => JsonScalar::Str,
                    // `staff: :boolean` declares the type and leaves the
                    // value nil until something writes one. Every reader
                    // here returns the scalar type itself, so there is
                    // no honest answer for an unwritten key — the
                    // declaration stays unclaimed and keeps warning.
                    _ => {
                        complete = false;
                        continue;
                    }
                };
                attrs.push(HasJsonAttr { name, scalar, default: default.clone() });
            }
        }
        if complete && !attrs.is_empty() {
            out.push(HasJsonDecl { span: expr.span, column, attrs });
        }
    }
    out
}

/// Synthesize the flat accessor triple for every schema key.
///
/// A custom method in the model body must win, and `push_user_methods`
/// runs after this — `push_synth_instance_method` does that check
/// itself, same dance as `push_typed_store_methods`.
pub(crate) fn push_has_json_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for decl in has_json_decls(&model.body) {
        for a in &decl.attrs {
            let flat = flat_name(&decl.column, &a.name);
            push_synth_instance_method(
                methods,
                model,
                flat.clone(),
                Vec::new(),
                read_body(&decl.column, a),
                Some(super::model_to_library::fn_sig(vec![], a.scalar.ty())),
                AccessorKind::Method,
                false,
            );
            // Rails' `key?` is `@data[key].present?`. On a boolean that
            // IS the value; on a string it is the non-empty test. An
            // integer's `present?` is unconditionally true (`0.present?`
            // is true in Ruby), so a predicate there would be a method
            // that always answers the same thing — not emitted, and no
            // corpus site asks for one. Widening is a deliberate later
            // step, same posture as typed_store's.
            let pred = match a.scalar {
                JsonScalar::Bool => Some(read_body(&decl.column, a)),
                JsonScalar::Str => Some(ne_empty(read_body(&decl.column, a))),
                JsonScalar::Int => None,
            };
            if let Some(body) = pred {
                push_synth_instance_method(
                    methods,
                    model,
                    Symbol::from(format!("{}?", flat.as_str())),
                    Vec::new(),
                    body,
                    Some(super::model_to_library::fn_sig(vec![], Ty::Bool)),
                    AccessorKind::Method,
                    false,
                );
            }
            // The writer is a COERCION BOUNDARY: Rails casts through an
            // ActiveModel type precisely because a form sends `"0"` /
            // `"true"` where the schema says boolean. The parameter is
            // `untyped` for the same reason Rails' own is — what a cast
            // accepts is not one type — and the body narrows it
            // immediately, so nothing downstream stays wide. (Same
            // reasoning, and the same `bool_cast`, as typed_store's.)
            let value = Symbol::from("value");
            push_synth_instance_method(
                methods,
                model,
                Symbol::from(format!("{}=", flat.as_str())),
                vec![Param::positional(value.clone())],
                write_body(&decl.column, a),
                Some(super::model_to_library::fn_sig(
                    vec![(value, Ty::Untyped)],
                    Ty::Nil,
                )),
                AccessorKind::Method,
                true,
            );
        }
    }
}

/// `<col>_<key>` — the flat accessor name. Prefixed by the column so
/// two `has_json` columns on one model can name the same key, and so
/// the synthesized name can't collide with an unrelated attribute.
pub(crate) fn flat_name(column: &Symbol, key: &Symbol) -> Symbol {
    Symbol::from(format!("{}_{}", column.as_str(), key.as_str()))
}

/// column → key → scalar type, over every model that declares one
/// CONSISTENTLY. Keyed by COLUMN like `enum_symbols`, and for the same
/// reason: resolving the model at every call site means typing the
/// receiver, and the schema is a property of the column anyway. A
/// column two models declare DIFFERENTLY is dropped entirely rather
/// than guessed at.
pub(crate) fn has_json_columns(
    models: &[Model],
) -> HashMap<String, HashMap<String, JsonScalar>> {
    let mut out: HashMap<String, HashMap<String, JsonScalar>> = HashMap::new();
    let mut conflicted: HashSet<String> = HashSet::new();
    for model in models {
        for decl in has_json_decls(&model.body) {
            let col = decl.column.as_str().to_string();
            if conflicted.contains(&col) {
                continue;
            }
            let schema: HashMap<String, JsonScalar> = decl
                .attrs
                .iter()
                .map(|a| (a.name.as_str().to_string(), a.scalar))
                .collect();
            match out.get(&col) {
                Some(existing) if *existing != schema => {
                    out.remove(&col);
                    conflicted.insert(col);
                }
                Some(_) => {}
                None => {
                    out.insert(col, schema);
                }
            }
        }
    }
    out
}

/// Rewrite every two-hop `<recv>.<col>.<key>` site to the one-hop flat
/// accessor the model now carries. The accessor object Rails returns
/// exists only between those two hops; erasing it here is what lets
/// every target see an ordinary typed method call.
pub fn apply_has_json_lowering(
    app: &mut crate::app::App,
) -> Vec<crate::diagnostic::Diagnostic> {
    let map = has_json_columns(&app.models);
    let mut diags = Vec::new();
    if map.is_empty() {
        return diags;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &map, &mut diags));
    for view in &mut app.views {
        rewrite(&mut view.body, &map, &mut diags);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &map, &mut diags);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &map, &mut diags);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &map, &mut diags);
        }
    }
    diags
}

fn rewrite(
    expr: &mut Expr,
    map: &HasJsonColumns,
    diags: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, map, diags));
    report_whole_column_assignment(expr, map, diags);
    match &mut *expr.node {
        // `x.settings.foo`, `x.settings.foo?`, `x.settings.foo = v` —
        // a plain attribute write ingests as a `foo=` send, so all
        // three shapes are the same rewrite.
        ExprNode::Send { recv: Some(inner), method, .. } => {
            if let Some(flat) = flatten(inner, method.as_str(), map) {
                *method = flat;
            }
        }
        // `x.settings.foo ||= v` — the compound forms keep an LValue.
        ExprNode::Assign { target: LValue::Attr { recv, name }, .. }
        | ExprNode::OpAssign { target: LValue::Attr { recv, name }, .. } => {
            if let Some(flat) = flatten(recv, name.as_str(), map) {
                *name = flat;
            }
        }
        _ => {}
    }
}

type HasJsonColumns = HashMap<String, HashMap<String, JsonScalar>>;

/// When `recv` is the `<base>.<col>` hop of a schema key named by
/// `method`, collapse `recv` to `<base>` and answer with the flat
/// accessor name. `method` keeps its `?`/`=` suffix, which the flat
/// name inherits.
fn flatten(recv: &mut Expr, method: &str, map: &HasJsonColumns) -> Option<Symbol> {
    let key = method.trim_end_matches(['?', '=']);
    let (col, base) = match &*recv.node {
        ExprNode::Send { recv: base, method: col, args, block: None, .. }
            if args.is_empty() =>
        {
            (col.clone(), base.clone())
        }
        _ => return None,
    };
    if !map.get(col.as_str())?.contains_key(key) {
        return None;
    }
    let flat = Symbol::from(format!("{}_{}", col.as_str(), method));
    match base {
        // An implicit-self hop inside the declaring model
        // (`settings.foo?`) collapses to an implicit-self flat call.
        None => {
            *recv = Expr::new(recv.span, ExprNode::SelfRef);
            Some(flat)
        }
        Some(base) => {
            *recv = base;
            Some(flat)
        }
    }
}

fn sym_lit(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

fn sp(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn schematized_json(method: &str, args: Vec<Expr>) -> Expr {
    sp(ExprNode::Send {
        recv: Some(sp(ExprNode::Const { path: vec![Symbol::from("SchematizedJson")] })),
        method: Symbol::from(method),
        args,
        block: None,
        parenthesized: true,
    })
}

/// `SchematizedJson.read_<ty>(@<col>, "<key>", <default>)`.
fn read_body(col: &Symbol, a: &HasJsonAttr) -> Expr {
    super::typing::with_ty(
        schematized_json(
            a.scalar.reader(),
            vec![
                sp(ExprNode::Ivar { name: col.clone() }),
                sp(ExprNode::Lit {
                    value: Literal::Str { value: a.name.as_str().to_string() },
                }),
                sp(ExprNode::Lit { value: a.default.clone() }),
            ],
        ),
        a.scalar.ty(),
    )
}

/// `@<col> = SchematizedJson.write_<ty>(@<col>, "<key>", <cast(value)>)`.
fn write_body(col: &Symbol, a: &HasJsonAttr) -> Expr {
    let value = sp(ExprNode::Var { id: VarId(0), name: Symbol::from("value") });
    let cast = match a.scalar {
        JsonScalar::Bool => super::typed_store::bool_cast(value),
        JsonScalar::Int => sp(ExprNode::Send {
            recv: Some(value),
            method: Symbol::from("to_i"),
            args: vec![],
            block: None,
            parenthesized: false,
        }),
        JsonScalar::Str => sp(ExprNode::Send {
            recv: Some(value),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        }),
    };
    sp(ExprNode::Assign {
        target: LValue::Ivar { name: col.clone() },
        value: schematized_json(
            a.scalar.writer(),
            vec![
                sp(ExprNode::Ivar { name: col.clone() }),
                sp(ExprNode::Lit {
                    value: Literal::Str { value: a.name.as_str().to_string() },
                }),
                cast,
            ],
        ),
    })
}

/// `<read> != ""` — a String key's `present?`.
fn ne_empty(read: Expr) -> Expr {
    super::typing::with_ty(
        sp(ExprNode::Send {
            recv: Some(read),
            method: Symbol::from("!="),
            args: vec![sp(ExprNode::Lit {
                value: Literal::Str { value: String::new() },
            })],
            block: None,
            parenthesized: false,
        }),
        Ty::Bool,
    )
}

/// ActiveRecord methods that take a column => value attribute hash —
/// the same census `enum_symbols` keys off, for the same reason.
const ATTR_METHODS: &[&str] = &[
    "new", "create", "create!", "build", "update", "update!", "update_columns",
    "assign_attributes", "update_attribute", "first_or_create",
    "first_or_create!", "first_or_initialize",
];

/// Assigning a has_json column a HASH — `account.update!(settings: { … })`,
/// Rails' `settings=` — is not modeled, and says so.
///
/// Rails overrides that writer to cast each supplied key through its
/// declared type. Here the column writer stays the plain serialized-text
/// one, because the same method is also where hydration lands
/// (`from_row` assigns the stored column straight through it) and only a
/// runtime type test could tell the two shapes apart — a test whose
/// untyped-Hash traversal no target's Hash surface resolves. So the
/// supported spelling is the per-key writer the declaration generates
/// (`account.settings_restrict = true`), and the unsupported one is a
/// diagnostic rather than a silent `Hash#to_s` in the column.
fn report_whole_column_assignment(
    expr: &Expr,
    map: &HasJsonColumns,
    diags: &mut Vec<crate::diagnostic::Diagnostic>,
) {
    let ExprNode::Send { method, args, .. } = &*expr.node else { return };
    // `record.settings = { … }` — a plain attribute write ingests as a
    // `settings=` send.
    if let Some(col) = method.as_str().strip_suffix('=') {
        if map.contains_key(col) && args.len() == 1 && is_hash(&args[0]) {
            diags.push(unmodeled(expr, col));
        }
        return;
    }
    if !ATTR_METHODS.contains(&method.as_str()) {
        return;
    }
    for arg in args {
        let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
        for (key, value) in entries {
            let Some(col) = sym_lit(key) else { continue };
            if map.contains_key(col.as_str()) && is_hash(value) {
                diags.push(unmodeled(expr, col.as_str()));
            }
        }
    }
}

fn is_hash(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Hash { .. })
}

fn unmodeled(expr: &Expr, column: &str) -> crate::diagnostic::Diagnostic {
    let mut d = crate::diagnostic::Diagnostic::unsupported(
        expr.span,
        None,
        "has_json",
        format!(
            "whole-Hash assignment to the `{column}` json column is not modeled \
             (Rails casts each key through the declared schema here); assign \
             through the per-key writer `{column}_<key>=` instead",
        ),
    );
    d.severity = crate::diagnostic::Severity::Warning;
    d
}
