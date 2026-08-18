//! Action Text — `has_rich_text` and the `ActionText::RichText` record
//! it hangs off.
//!
//! # Where the pieces live
//!
//! Rails splits Action Text three ways, and so does this:
//!
//! * `ActionText::RichText` is a MODEL. It has a table
//!   (`action_text_rich_texts`), a polymorphic `belongs_to :record`,
//!   and rows that are found, built and saved like any other. So it is
//!   synthesized here as an ordinary [`Model`] and pushed onto
//!   `app.models` — after which columns, `where`, hydration,
//!   persistence and per-target emit all arrive from the machinery
//!   that already exists. Nothing about it is special-cased downstream.
//!
//! * `ActionText::Content` is a VALUE — an HTML fragment with a
//!   plain-text projection. It has no table, so it lives in the
//!   framework runtime (`runtime/ruby/action_text.rb`) and this pass
//!   only constructs it.
//!
//! * `has_rich_text :body` is a MACRO. It expands to an association
//!   plus five methods, and that expansion is what
//!   [`push_rich_text_methods`] performs — the same shape
//!   `has_secure_password` and `typed_store` get, in the same slot of
//!   `build_methods`.
//!
//! # The association is written out, not declared
//!
//! Rails' macro emits `has_one :rich_text_body, -> { where(name:
//! "body") }, as: :record`. That declaration cannot be handed to
//! [`Association::HasOne`] as-is: the `has_one` synthesizer has no
//! scope parameter, so the reader it builds would find the FIRST rich
//! text on the record regardless of name — right for a model with one
//! rich-text attribute and silently wrong for a model with two. The
//! reader is therefore written out here with all three scope terms
//! (`record_id`, `record_type`, `name`), which is also the shape that
//! makes the `name` half visible in the IR rather than buried in a
//! lambda.
//!
//! # What Rails does that this does not
//!
//! `encrypted:` (a different record class) and `strict_loading:` are
//! not modeled; `store_if_blank: false` (which marks a blank body for
//! destruction) is not modeled. Each would be a straightforward
//! addition to the expansion below; none appears in the corpus, and
//! silently accepting the option while ignoring it is exactly the
//! failure mode the unclaimed-DSL report exists to prevent — so a
//! declaration carrying one still warns.

use crate::dialect::{
    AccessorKind, Association, MethodDef, MethodReceiver, Model, ModelBodyItem, Param,
};
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TableRef, VarId};
use crate::span::Span;
use crate::ty::{Row, Ty};
use crate::App;

use super::model_to_library::{
    class_const, fn_sig, lit_str, lit_sym, model_defines_instance_method, nil_lit, seq, var_ref,
};

/// The table Rails' Action Text migration creates. A schema without it
/// means the app declared `has_rich_text` but never installed Action
/// Text — nothing to lower against, so the pass stands down and the
/// declaration reports as unclaimed like any other unmodeled DSL.
pub const RECORD_TABLE: &str = "action_text_rich_texts";

/// `ActionText::RichText` — the record class, named exactly as Rails
/// names it so app code that refers to it by name resolves.
pub fn record_class() -> ClassId {
    ClassId(Symbol::from("ActionText::RichText"))
}

/// `ActionText::Content` — the coder the `body` column reads back as.
pub fn content_class() -> ClassId {
    ClassId(Symbol::from("ActionText::Content"))
}

/// Every `has_rich_text :name` in a model body, in declaration order,
/// paired with the declaration's span.
///
/// Only the single-symbol form is claimed. A declaration carrying
/// options (`encrypted: true`, `store_if_blank: false`) changes what
/// the expansion must be, and this pass implements none of them — so
/// it is left unclaimed, and `report_unclaimed_unknowns` names it.
pub fn rich_text_attrs(model: &Model) -> Vec<(Span, Symbol)> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "has_rich_text" || args.len() != 1 {
            continue;
        }
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
            out.push((expr.span, Symbol::from(value.as_str())));
        }
    }
    out
}

/// Whether `model` is the synthesized record class.
pub fn is_record_model(model: &Model) -> bool {
    model.name == record_class()
}

/// Does any model in the app declare a rich-text attribute?
fn app_uses_rich_text(app: &App) -> bool {
    app.models.iter().any(|m| !rich_text_attrs(m).is_empty())
}

/// Push `ActionText::RichText` onto `app.models` when some model
/// declares `has_rich_text` and the schema carries its table.
///
/// Called from ingest assembly rather than from a lowering pass
/// because everything downstream — the association graph, the scope
/// registry, the model loop in `analyze`, every emitter's model file
/// list — reads `app.models`. A model that appears later than that is
/// a model half the pipeline cannot see.
///
/// Idempotent, and yields to a real `app/models/action_text/rich_text.rb`
/// if an app ever ships one.
pub fn synthesize_record_model(app: &mut App) {
    if !app_uses_rich_text(app) {
        return;
    }
    let class = record_class();
    if app.models.iter().any(|m| m.name == class) {
        return;
    }
    if !app.schema.tables.contains_key(&Symbol::from(RECORD_TABLE)) {
        return;
    }
    // `belongs_to :record, polymorphic: true` is declared (rather than
    // written out) because the has_one side is what needed the scope;
    // this side is an ordinary polymorphic belongs_to and the existing
    // synthesizer produces exactly Rails' reader.
    //
    // `polymorphic_targets` stays empty on purpose: it is filled by
    // `resolve_polymorphic_targets` from the inverse `as:` declarations,
    // and no model declares `has_one … as: :record` in source — the
    // owner side is this file's own expansion. The consequence is that
    // `rich_text.record` reads as gradual rather than as a union of the
    // models that use it, which is the honest type for a column that
    // can name any of them.
    let body = vec![ModelBodyItem::Association {
        assoc: Association::BelongsTo {
            name: Symbol::from("record"),
            target: ClassId(Symbol::from("Record")),
            foreign_key: Symbol::from("record_id"),
            optional: false,
            polymorphic: true,
            polymorphic_targets: Vec::new(),
            default: None,
        },
        leading_comments: Vec::new(),
        leading_blank_line: false,
        span: Span::synthetic(),
    }];
    app.models.push(Model {
        name: class,
        parent: Some(ClassId(Symbol::from("ApplicationRecord"))),
        table: TableRef(Symbol::from(RECORD_TABLE)),
        primary_key: None,
        attributes: Row::closed(),
        body,
        span: Span::synthetic(),
        enums: indexmap::IndexMap::new(),
    });
}

/// Synthesize the Action Text method surface onto `model`.
///
/// Two disjoint jobs behind one entry point, because they are two
/// halves of one feature and splitting them across the pipeline is how
/// they drift: the record class gets its `body`-as-Content coder, and
/// a declaring model gets its `has_rich_text` expansion.
pub(crate) fn push_rich_text_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    if is_record_model(model) {
        push_record_methods(methods, model);
        return;
    }
    for (span, attr) in rich_text_attrs(model) {
        let before = methods.len();
        push_owner_methods(methods, model, &attr);
        for m in &mut methods[before..] {
            m.body.inherit_span(span);
        }
    }
}

// ── the record class ───────────────────────────────────────────

/// `serialize :body, coder: ActionText::Content`, done as a reader
/// override.
///
/// The column holds HTML text and the schema synthesizer has already
/// produced a `String` accessor pair over `@body`. Rails' `serialize`
/// keeps that storage and changes what the ATTRIBUTE reads back as, so
/// that is what happens here: the reader is replaced with one that
/// wraps `@body`, and the writer is replaced with one that accepts a
/// String or a Content and stores the markup.
///
/// The reader is deliberately NOT memoized. A memo would need
/// invalidating on every path that writes `@body` — the writer,
/// hydration, `[]=`, `update`, the adapter — and a stale Content is a
/// silently wrong message body. Constructing one is a field read and
/// an object allocation; the scanner inside it runs only when someone
/// asks for plain text.
fn push_record_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let body_col = Symbol::from("body");
    let content_ty = Ty::Class { id: content_class(), args: vec![] };

    // def body; ActionText::Content.new(@body); end
    replace_or_push(
        methods,
        model,
        MethodDef {
            name: body_col.clone(),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: content_new(ivar("body")),
            signature: Some(fn_sig(vec![], content_ty.clone())),
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            // `Method`, not `AttributeReader`: the strict targets read
            // `AttributeReader` as "declare a field of this type", and
            // the field here is the String, not the Content.
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        },
    );

    // def body=(value); @body = value.to_s; end
    //
    // `to_s` is what makes the writer accept both spellings Rails
    // accepts — `record.body = "<div>hi</div>"` (the params path) and
    // `record.body = other.body` (a Content) — without a type test.
    let value = Symbol::from("value");
    replace_or_push(
        methods,
        model,
        MethodDef {
            name: Symbol::from("body="),
            receiver: MethodReceiver::Instance,
            params: vec![Param::positional(value.clone())],
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: body_col },
                    value: no_arg_send(var_ref(value.clone()), "to_s"),
                },
            ),
            signature: Some(fn_sig(vec![(value, Ty::Str)], Ty::Str)),
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            // Still the writer half of the `body` field — strict
            // targets derive the String field from this pair member.
            kind: AccessorKind::AttributeWriter,
            is_async: false,
            mutates_self: true,
            block_param: None,
        },
    );

    // `delegate :to_s, :nil?, to: :body` plus RichText's own
    // `to_plain_text` and the `blank?`/`empty?`/`present?` trio it
    // delegates to that. Each forwards to the Content the reader
    // builds, which is why they are three-token bodies rather than
    // re-derivations.
    // RichText's predicates delegate to `to_plain_text`, not to the
    // markup: a body of `<div></div>` is `blank?` in Rails even though
    // the column is not empty. Content already answers each of these on
    // plain text, so every delegation is name-for-name.
    for (name, ret) in [
        ("to_s", Ty::Str),
        ("to_plain_text", Ty::Str),
        ("to_html", Ty::Str),
        ("blank?", Ty::Bool),
        ("empty?", Ty::Bool),
        ("present?", Ty::Bool),
    ] {
        super::model_to_library::push_synth_instance_method(
            methods,
            model,
            Symbol::from(name),
            Vec::new(),
            no_arg_send(content_new(ivar("body")), name),
            Some(fn_sig(vec![], ret)),
            AccessorKind::Method,
            false,
        );
    }

    // `to_trix_html` — what the editor's hidden input carries, and the
    // one delegation that is not name-for-name.
    //
    // Rails renders attachment nodes into `<figure
    // data-trix-attachment=…>` previews on the way in so the editor can
    // show them; that conversion needs the attachment DEREFERENCED (a
    // blob's URL, a mention's avatar), which is the signed-GlobalID
    // step Content deliberately does not take. So this hands back the
    // stored markup unchanged: the editor loads the text correctly and
    // renders attachment nodes as bare elements rather than previews.
    super::model_to_library::push_synth_instance_method(
        methods,
        model,
        Symbol::from("to_trix_html"),
        Vec::new(),
        no_arg_send(content_new(ivar("body")), "to_html"),
        Some(fn_sig(vec![], Ty::Str)),
        AccessorKind::Method,
        false,
    );
}

// ── the declaring model ────────────────────────────────────────

/// Rails' `has_rich_text :body` expansion, method for method:
///
/// ```ruby
/// def rich_text_body                       # the has_one, scoped by name
/// def build_rich_text_body                 # the association's build_
/// def body        = rich_text_body || build_rich_text_body
/// def body?       = !rich_text_body.nil?
/// def body=(v)    = body.body = v
/// ```
///
/// plus the `autosave: true` half, as an `after_save` that writes the
/// built record through once the owner has an id.
fn push_owner_methods(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    let assoc = Symbol::from(format!("rich_text_{}", attr.as_str()));
    let builder = Symbol::from(format!("build_rich_text_{}", attr.as_str()));
    let cache = Symbol::from(format!("__rich_text_{}", attr.as_str()));
    let loaded = Symbol::from(format!("__rich_text_{}_loaded", attr.as_str()));
    let record_ty = Ty::Class { id: record_class(), args: vec![] };
    let maybe_record = Ty::Union { variants: vec![record_ty.clone(), Ty::Nil] };
    let push = super::model_to_library::push_synth_instance_method;

    // The association reader, with Rails' load-once semantics. The
    // separate `_loaded` flag is what keeps "no row in the database"
    // from re-querying on every read — a nil cache alone cannot tell
    // "not looked yet" from "looked, found nothing", and this reader
    // runs once per message per render.
    //
    // The unsaved owner short-circuits before the query, as Rails does:
    // no row can point at an id that has not been assigned yet, so
    // `message = Message.new; message.body = "…"` would otherwise open
    // with a guaranteed-empty SELECT on every create.
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
                        assign_ivar(&cache, first_rich_text(model, attr)),
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

    // `build_rich_text_<attr>` — a new record already pointed at this
    // owner, and (as in Rails) installed as the association's target so
    // the next read returns it rather than re-querying.
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
            attr_assign(var_ref(record_var.clone()), "body", lit_str(String::new())),
            assign_ivar(&cache, var_ref(record_var.clone())),
            assign_ivar(&loaded, lit_true()),
            var_ref(record_var),
        ]),
        Some(fn_sig(vec![], record_ty.clone())),
        AccessorKind::Method,
        true,
    );

    // `message.body` — never nil, which is why the reader can be typed
    // as the record rather than as a nullable one. This is the method
    // that makes `message.body.to_plain_text` work on a message that
    // has no rich text row at all.
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

    // `message.body?`
    push(
        methods,
        model,
        Symbol::from(format!("{}?", attr.as_str())),
        Vec::new(),
        no_arg_send(no_arg_send(self_send(&assoc), "nil?"), "!"),
        Some(fn_sig(vec![], Ty::Bool)),
        AccessorKind::Method,
        true,
    );

    // `message.body = "<div>hi</div>"` — the params path. Assigns
    // THROUGH the reader, so a message with no row yet gets one built
    // and the markup lands on it.
    let value = Symbol::from("value");
    push(
        methods,
        model,
        Symbol::from(format!("{}=", attr.as_str())),
        vec![Param::positional(value.clone())],
        attr_assign(self_send(attr), "body", var_ref(value.clone())),
        Some(fn_sig(vec![(value, Ty::Str)], Ty::Str)),
        AccessorKind::Method,
        true,
    );

    // `autosave: true`. The owner's id is only known after its own
    // insert, so the built record's `record_id` is (re)stamped here
    // rather than at build time — a create assigns `body` before the
    // owner has an id, and a rich text row pointing at id 0 is a row
    // nothing can ever find.
    push(
        methods,
        model,
        Symbol::from(format!("_save_rich_text_{}", attr.as_str())),
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
        self_send(&Symbol::from(format!("_save_rich_text_{}", attr.as_str()))),
    );
}

/// `ActionText::RichText.where(record_id: @id, record_type: "<Owner>",
/// name: "<attr>").first` — all three scope terms the macro's
/// `has_one … -> { where(name: name) }, as: :record` implies.
fn first_rich_text(model: &Model, attr: &Symbol) -> Expr {
    let entries = vec![
        (lit_sym(Symbol::from("record_id")), ivar("id")),
        (
            lit_sym(Symbol::from("record_type")),
            lit_str(model.name.0.as_str().to_string()),
        ),
        (lit_sym(Symbol::from("name")), lit_str(attr.as_str().to_string())),
    ];
    let query = Expr::new(
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
    );
    no_arg_send(query, "first")
}

// ── small builders ─────────────────────────────────────────────

/// Replace a synthesized method of the same name, or push it. Unlike
/// `push_synth_instance_method` (which yields to whatever is already
/// there), this one WINS — the schema synthesizer has already produced
/// a plain `body` accessor pair and overriding it is the whole point.
/// A method the model's own source defines still wins over both, since
/// `push_user_methods` runs after this and skips names already taken.
fn replace_or_push(methods: &mut Vec<MethodDef>, model: &Model, def: MethodDef) {
    if model_defines_instance_method(model, &def.name) {
        return;
    }
    match methods
        .iter()
        .position(|m| m.receiver == MethodReceiver::Instance && m.name == def.name)
    {
        Some(i) => methods[i] = def,
        None => methods.push(def),
    }
}

fn content_new(arg: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&content_class())),
            method: Symbol::from("new"),
            args: vec![arg],
            block: None,
            parenthesized: true,
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

// ── the preload scopes ─────────────────────────────────────────

/// The two scopes Rails' macro declares beside the association:
/// `with_rich_text_<attr>` and `with_rich_text_<attr>_and_embeds`.
///
/// Shared by the analyzer (which registers them as relation-returning
/// class methods so call sites type) and the ruby emit seam (which
/// gives them bodies), so the two cannot disagree about which names
/// exist.
pub fn preload_scope_names(model: &Model) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (_span, attr) in rich_text_attrs(model) {
        out.push(Symbol::from(format!("with_rich_text_{}", attr.as_str())));
        out.push(Symbol::from(format!(
            "with_rich_text_{}_and_embeds",
            attr.as_str()
        )));
    }
    out
}

/// Give the preload scopes bodies: `def self.with_rich_text_body(__rel
/// = ActiveRecord::Relation.new(self)) = __rel`.
///
/// IDENTITY, and that is the whole implementation. In Rails these are
/// `includes(...)` — a QUERY PLAN hint that changes how many round
/// trips a page costs and nothing about what it returns. The rich-text
/// reader synthesized above fetches per record, so the hint has nothing
/// to attach to and the scope has nothing to do but pass the relation
/// along.
///
/// The cost is real and is stated here rather than hidden: a page that
/// renders N messages issues N rich-text queries where Rails issues
/// one. That is an N+1, not a wrong answer — and the alternative,
/// dropping the method, turns a performance difference into a
/// NoMethodError on every call site that chains through it.
///
/// Ruby-family only, for the same reason `push_scope_methods` is:
/// `ActiveRecord::Relation` is a CRuby/JRuby runtime class.
pub(crate) fn push_preload_scope_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let rel = Symbol::from("__rel");
    for name in preload_scope_names(model) {
        if methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Class && m.name == name)
        {
            continue;
        }
        methods.push(MethodDef {
            name,
            receiver: MethodReceiver::Class,
            params: vec![Param::with_default(
                rel.clone(),
                super::model_to_library::relation_new_self(),
            )],
            body: var_ref(rel.clone()),
            // No signature, matching `push_scope_methods` — a declared
            // `scope` leaves this None too. A `Ty::Relation` in the
            // signature is a relation REACHING EMIT, which every
            // emitter reports as unsupported (chains are meant to
            // specialize to SQL before that point); typing these four
            // slots added four such reports for a method whose body is
            // one identity read. The analyzer's own registration
            // (`registry`-side, from `preload_scope_names`) is what
            // types the CALL SITE, and that is the half that matters.
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

/// `@id.nil? || @id == 0` — "this owner has never been saved".
///
/// The `== 0` half is the same unset-id sentinel the synthesized
/// `belongs_to` presence checks use (`validations::inline_belongs_to_check`);
/// a freshly constructed record's `id` is 0, not nil, because
/// `initialize` defaults every integer column.
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
