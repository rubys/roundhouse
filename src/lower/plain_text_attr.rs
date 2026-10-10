//! Action Text–style **named plain-text association** — Rails'
//! `has_markdown :name` and the `ActionText::Markdown` record it hangs
//! off.
//!
//! # Where the pieces live
//!
//! Same three-way split as [`super::rich_text`], with different storage:
//!
//! * `ActionText::Markdown` is a MODEL (table `action_text_markdowns`,
//!   polymorphic `belongs_to :record`, raw attribute `content`). Apps
//!   may ship it under `lib/` (ordinary ingest) or leave it absent —
//!   [`synthesize_record_model`] fills the gap when a declaration is
//!   present and the schema carries the table.
//! * There is no Content-coder value type: `content` is plain text.
//! * `has_markdown :body` is a MACRO. Expansion is
//!   [`push_plain_text_methods`] — association scoped by owner/name,
//!   reader/predicate/writer, ordinary autosave, dependent destroy,
//!   and the two preload scopes.
//!
//! # Why a dedicated pass (not concern `class_eval`)
//!
//! Rails' `ActionText::HasMarkdown` builds the reader trio with an
//! interpolatable `class_eval` heredoc, then declares
//! `has_one :"markdown_#{name}", -> { where(name: name) }, …` and the
//! `with_markdown_*` scopes. The class_eval expander aborts on those
//! leftover association/scope forms (fail-closed). Expanding only the
//! rewritten methods would quiet diagnostics without association
//! semantics — the opposite of invariant 6. This pass claims the bare
//! `has_markdown :sym` Unknown send the same way `has_rich_text` is
//! claimed: compile-time-resolvable association+storage, not string
//! eval.
//!
//! # Differences from `has_rich_text`
//!
//! | | Markdown / plain-text | RichText |
//! |---|---|---|
//! | Storage column | `content` | `body` (+ Content coder) |
//! | Assoc prefix | `markdown_<name>` | `rich_text_<name>` |
//! | Table | `action_text_markdowns` | `action_text_rich_texts` |
//! | Autosave blanks | **Ordinary** — empty content and read-materialized children save | Suppresses new blank rows |
//! | Predicate | `markdown_<name>.present?` | `!rich_text_<name>.nil?` |
//!
//! Options beyond the bare symbol form (`strict_loading:`, …) stay
//! unclaimed.

use crate::dialect::{
    AccessorKind, Association, MethodDef, MethodReceiver, Model, ModelBodyItem, Param,
};
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TableRef, VarId};
use crate::span::Span;
use crate::ty::Ty;
use crate::App;

use super::model_to_library::{
    class_const, fn_sig, lit_str, lit_sym, nil_lit, seq, var_ref,
};

/// Table Rails' Action Text Markdown migration creates.
pub const RECORD_TABLE: &str = "action_text_markdowns";

/// Whether the schema carries the Markdown storage table. Claiming and
/// method expansion both require it — without the table there is no
/// honest runtime for `ActionText::Markdown.where` / `.new`.
pub fn record_table_present(schema: &crate::schema::Schema) -> bool {
    schema.tables.contains_key(&crate::ident::Symbol::from(RECORD_TABLE))
}

/// Rails API name for the declaration. Recognition only — file/test
/// stems stay abstract (`plain_text_attr`).
pub const DECL_MACRO: &str = "has_markdown";

/// `ActionText::Markdown` — the record class.
pub fn record_class() -> ClassId {
    ClassId(Symbol::from("ActionText::Markdown"))
}

/// Every bare `has_markdown :name` in a model body, in declaration order.
///
/// Only the single-symbol form is claimed. A declaration carrying
/// options (`strict_loading:`) changes what the expansion must be, and
/// this pass implements none of them — left unclaimed.
pub fn plain_text_attrs(model: &Model) -> Vec<(Span, Symbol)> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != DECL_MACRO || args.len() != 1 {
            continue;
        }
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
            out.push((expr.span, Symbol::from(value.as_str())));
        }
    }
    out
}

/// True when `method`/`args` is a bare form this pass claims — used by
/// ingest to skip concern `class_eval` expansion (and its fail-closed
/// gap) so the Unknown send reaches this lowerer.
pub fn claims_call(method: &Symbol, args: &[Expr]) -> bool {
    if method.as_str() != DECL_MACRO || args.len() != 1 {
        return false;
    }
    matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
}

/// Whether `model` is the (ingested or synthesized) record class.
pub fn is_record_model(model: &Model) -> bool {
    model.name == record_class()
}

fn app_uses_plain_text(app: &App) -> bool {
    app.models.iter().any(|m| !plain_text_attrs(m).is_empty())
}

/// Push `ActionText::Markdown` onto `app.models` when some model
/// declares `has_markdown` and the schema carries its table, unless an
/// app (or prior ingest) already shipped the class.
pub fn synthesize_record_model(app: &mut App) {
    if !app_uses_plain_text(app) {
        return;
    }
    let class = record_class();
    if app.models.iter().any(|m| m.name == class) {
        return;
    }
    let Some(table) = app.schema.tables.get(&Symbol::from(RECORD_TABLE)) else { return };
    let attributes = crate::ingest::model::row_from_table(table);
    let body = vec![ModelBodyItem::Association {
        assoc: Association::BelongsTo {
            name: Symbol::from("record"),
            target: ClassId(Symbol::from("Record")),
            foreign_key: Symbol::from("record_id"),
            optional: false,
            polymorphic: true,
            polymorphic_targets: Vec::new(),
            default: None,
            touch: None,
            foreign_type: None,
            primary_key: None,
        },
        leading_comments: Vec::new(),
        leading_blank_line: false,
        span: Span::synthetic(),
    }];
    app.models.push(Model {
        sti_subclass_names: Vec::new(),
        name: class,
        parent: Some(ClassId(Symbol::from("ApplicationRecord"))),
        parent_span: Default::default(),
        table: TableRef(Symbol::from(RECORD_TABLE)),
        primary_key: None,
        attributes,
        body,
        span: Span::synthetic(),
        enums: indexmap::IndexMap::new(),
        enum_defaults: indexmap::IndexMap::new(),
        class_attr_defaults: indexmap::IndexMap::new(),
        lexical_json_shadow: false,
    });
}

/// Expand `has_markdown` onto declaring models. The record class needs
/// no Content-coder overrides — `content` is a plain String column.
/// No-ops when the backing table is absent so claim/expansion stay
/// paired with storage (Invariant 6).
pub(crate) fn push_plain_text_methods(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    schema: &crate::schema::Schema,
) {
    if is_record_model(model) || !record_table_present(schema) {
        return;
    }
    for (span, attr) in plain_text_attrs(model) {
        let before = methods.len();
        push_owner_methods(methods, model, &attr);
        for m in &mut methods[before..] {
            m.body.inherit_span(span);
        }
    }
}

/// Rails' `has_markdown :body` expansion, method for method:
///
/// ```ruby
/// def markdown_body
/// def build_markdown_body
/// def body        = markdown_body || build_markdown_body
/// def body?       = markdown_body.present?
/// def body=(v)    = body.content = v
/// ```
///
/// plus ordinary `autosave: true` (including blank content) and
/// `dependent: :destroy`.
fn push_owner_methods(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    let assoc = Symbol::from(format!("markdown_{}", attr.as_str()));
    let builder = Symbol::from(format!("build_markdown_{}", attr.as_str()));
    let cache = Symbol::from(format!("__markdown_{}", attr.as_str()));
    let loaded = Symbol::from(format!("__markdown_{}_loaded", attr.as_str()));
    let record_ty = Ty::Class { id: record_class(), args: vec![].into() };
    let maybe_record = Ty::Union { variants: vec![record_ty.clone(), Ty::Nil].into() };
    let push = super::model_to_library::push_synth_instance_method;

    push(
        methods,
        model,
        assoc.clone(),
        Vec::new(),
        seq(vec![
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: Expr::new(
                        Span::synthetic(),
                        ExprNode::BoolOp {
                            op: BoolOpKind::And,
                            surface: BoolOpSurface::default(),
                            left: no_arg_send(ivar(loaded.as_str()), "!"),
                            right: no_arg_send(unsaved_owner(), "!"),
                        },
                    ),
                    then_branch: seq(vec![
                        assign_ivar(&loaded, lit_true()),
                        assign_ivar(&cache, first_row(model, attr)),
                    ]),
                    else_branch: nil_lit(),
                },
            ),
            ivar(cache.as_str()),
        ]),
        Some(fn_sig(vec![], maybe_record.clone())),
        AccessorKind::Method,
        true,
    );

    let record_var = Symbol::from("record");
    push(
        methods,
        model,
        builder.clone(),
        Vec::new(),
        seq(vec![
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: record_var.clone() },
                    value: no_arg_send(class_const(&record_class()), "new"),
                },
            ),
            attr_assign(var_ref(record_var.clone()), "record_id", ivar("id")),
            attr_assign(
                var_ref(record_var.clone()),
                "record_type",
                lit_str(model.name.0.as_str().to_string()),
            ),
            attr_assign(
                var_ref(record_var.clone()),
                "name",
                lit_str(attr.as_str().to_string()),
            ),
            attr_assign(var_ref(record_var.clone()), "content", lit_str(String::new())),
            assign_ivar(&cache, var_ref(record_var.clone())),
            assign_ivar(&loaded, lit_true()),
            var_ref(record_var),
        ]),
        Some(fn_sig(vec![], record_ty.clone())),
        AccessorKind::Method,
        true,
    );

    let rec = Symbol::from("rec");
    push(
        methods,
        model,
        preload_setter_name(attr),
        vec![Param::positional(rec.clone())],
        seq(vec![
            assign_ivar(&cache, var_ref(rec.clone())),
            assign_ivar(&loaded, lit_true()),
            nil_lit(),
        ]),
        Some(fn_sig(vec![(rec, maybe_record.clone())], Ty::Nil)),
        AccessorKind::Method,
        true,
    );

    push(
        methods,
        model,
        attr.clone(),
        Vec::new(),
        Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: BoolOpKind::Or,
                surface: BoolOpSurface::default(),
                left: self_send(&assoc),
                right: self_send(&builder),
            },
        ),
        Some(fn_sig(vec![], record_ty)),
        AccessorKind::Method,
        true,
    );

    // `markdown_<attr>.present?` — Rails/HasMarkdown spelling (not `!nil?`).
    push(
        methods,
        model,
        Symbol::from(format!("{}?", attr.as_str())),
        Vec::new(),
        no_arg_send(self_send(&assoc), "present?"),
        Some(fn_sig(vec![], Ty::Bool)),
        AccessorKind::Method,
        true,
    );

    let value = Symbol::from("value");
    push(
        methods,
        model,
        Symbol::from(format!("{}=", attr.as_str())),
        vec![Param::positional(value.clone())],
        attr_assign(self_send(attr), "content", var_ref(value.clone())),
        Some(fn_sig(vec![(value, Ty::Str)], Ty::Str)),
        AccessorKind::Method,
        true,
    );

    // Ordinary autosave: save whenever a child is in the cache, including
    // empty content and read-materialized rows. Do not inherit RichText's
    // blank-row suppression.
    push(
        methods,
        model,
        Symbol::from(format!("_save_markdown_{}", attr.as_str())),
        Vec::new(),
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: no_arg_send(ivar(cache.as_str()), "nil?"),
                then_branch: nil_lit(),
                else_branch: seq(vec![
                    attr_assign(ivar(cache.as_str()), "record_id", ivar("id")),
                    no_arg_send(ivar(cache.as_str()), "save"),
                ]),
            },
        ),
        Some(fn_sig(vec![], Ty::Nil)),
        AccessorKind::Method,
        true,
    );
    super::model_to_library::markers::fold_into_or_push(
        methods,
        model,
        "after_save",
        self_send(&Symbol::from(format!("_save_markdown_{}", attr.as_str()))),
    );

    push(
        methods,
        model,
        Symbol::from(format!("_destroy_markdown_{}", attr.as_str())),
        Vec::new(),
        seq(vec![each_destroy(rows(model, attr)), nil_lit()]),
        Some(fn_sig(vec![], Ty::Nil)),
        AccessorKind::Method,
        true,
    );
    super::model_to_library::markers::fold_into_or_push(
        methods,
        model,
        "before_destroy",
        self_send(&Symbol::from(format!("_destroy_markdown_{}", attr.as_str()))),
    );
}

fn first_row(model: &Model, attr: &Symbol) -> Expr {
    no_arg_send(rows(model, attr), "first")
}

fn rows(model: &Model, attr: &Symbol) -> Expr {
    let entries = vec![
        (lit_sym(Symbol::from("record_id")), ivar("id")),
        (
            lit_sym(Symbol::from("record_type")),
            lit_str(model.name.0.as_str().to_string()),
        ),
        (lit_sym(Symbol::from("name")), lit_str(attr.as_str().to_string())),
    ];
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&record_class())),
            method: Symbol::from("where"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Hash { entries, kwargs: true },
            )],
            block: None,
            parenthesized: true,
        },
    )
}

fn each_destroy(rows: Expr) -> Expr {
    let var = Symbol::from("row");
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(),
            params: vec![var.clone()],
            rest_param: None,
            block_param: None,
            body: no_arg_send(var_ref(var), "destroy"),
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(rows),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    )
}

fn ivar(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from(name) })
}

fn no_arg_send(recv: Expr, method: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn self_send(method: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn assign_ivar(name: &Symbol, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign { target: LValue::Ivar { name: name.clone() }, value },
    )
}

fn attr_assign(recv: Expr, name: &str, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Attr { recv, name: Symbol::from(name) },
            value,
        },
    )
}

fn lit_true() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: true } })
}

pub fn preload_scope_names(model: &Model) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (_span, attr) in plain_text_attrs(model) {
        out.push(Symbol::from(format!("with_markdown_{}", attr.as_str())));
        out.push(Symbol::from(format!(
            "with_markdown_{}_and_embeds",
            attr.as_str()
        )));
    }
    out
}

pub fn preload_scopes(model: &Model) -> Vec<(Symbol, Symbol)> {
    let mut out = Vec::new();
    for (_span, attr) in plain_text_attrs(model) {
        let assoc = Symbol::from(format!("markdown_{}", attr.as_str()));
        out.push((
            Symbol::from(format!("with_markdown_{}", attr.as_str())),
            assoc.clone(),
        ));
        out.push((
            Symbol::from(format!("with_markdown_{}_and_embeds", attr.as_str())),
            assoc,
        ));
    }
    out
}

pub fn preload_setter_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_preload_markdown_{}", attr.as_str()))
}

pub(crate) fn push_preload_scope_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let rel = Symbol::from("__rel");
    for (name, assoc) in preload_scopes(model) {
        if methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Class && m.name == name)
        {
            continue;
        }
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name,
            receiver: MethodReceiver::Class,
            params: vec![Param::with_default(
                rel.clone(),
                super::model_to_library::relation_new_self(),
            )],
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(var_ref(rel.clone())),
                    method: Symbol::from("preload"),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: assoc.clone() } },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
            signature: None,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

fn unsaved_owner() -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: BoolOpKind::Or,
            surface: BoolOpSurface::default(),
            left: no_arg_send(ivar("id"), "nil?"),
            right: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(ivar("id")),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Int { value: 0 } },
                    )],
                    block: None,
                    parenthesized: false,
                },
            ),
        },
    )
}
