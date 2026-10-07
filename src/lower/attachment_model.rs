//! `ActiveStorage::Attachment` — the join-row MODEL behind every
//! `has_one_attached` / `has_many_attached` reader.
//!
//! # Where the pieces live
//!
//! Active Storage splits three ways, and so does this codebase:
//!
//! * `ActiveStorage::Attachment` is a MODEL. It has a table
//!   (`active_storage_attachments`), a polymorphic `belongs_to :record`,
//!   and rows that are found, built and saved like any other. So it is
//!   synthesized here as an ordinary [`Model`] and pushed onto
//!   `app.models` — after which columns, `where`, hydration,
//!   persistence and per-target emit all arrive from the machinery
//!   that already exists. Nothing about it is special-cased downstream.
//!
//! * `ActiveStorage::Blob` / `Attached` / `AttachedMany` are VALUE /
//!   proxy types with no table of their own (or, for Blob, a table
//!   answered by the runtime with raw SQL). They live in
//!   `runtime/ruby/active_storage.rb`.
//!
//! * `has_one_attached` / `has_many_attached` are MACROS expanded by
//!   [`super::attached`].
//!
//! The emit seam already anchors `ActiveStorage` to the runtime module
//! and comments that `Attachment` is *not* there — it resolves under
//! `app/models/` once this pass has pushed it. App code that writes
//! `ActiveStorage::Attachment.find_by!(…)` needs that constant to be a
//! real model, not an unsupported name.
//!
//! # What this does not model
//!
//! A `belongs_to :blob` association is deliberately omitted:
//! `ActiveStorage::Blob` is a runtime class, not an `app.models` entry,
//! so a declared association would name a target the association
//! graph cannot resolve. Instance methods (`url`, `filename`,
//! `content_type`) reach the blob through `ActiveStorage::Blob.find`
//! instead.

use crate::dialect::{AccessorKind, Association, MethodDef, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TableRef, VarId};
use crate::span::Span;
use crate::ty::Ty;
use crate::App;

use super::model_to_library::{class_const, fn_sig, nil_lit, seq, var_ref};

/// The table Rails' Active Storage migration creates.
pub const RECORD_TABLE: &str = "active_storage_attachments";

/// `ActiveStorage::Attachment` — named exactly as Rails names it.
pub fn record_class() -> ClassId {
    ClassId(Symbol::from("ActiveStorage::Attachment"))
}

/// Whether `model` is the synthesized (or app-shipped) Attachment class.
pub fn is_attachment_model(model: &Model) -> bool {
    model.name == record_class()
}

/// True when the schema carries the attachments table.
pub fn record_table_present(schema: &crate::schema::Schema) -> bool {
    schema.tables.contains_key(&Symbol::from(RECORD_TABLE))
}

/// Push `ActiveStorage::Attachment` onto `app.models` when the schema
/// carries its table.
///
/// Called from ingest assembly rather than from a lowering pass
/// because everything downstream — the association graph, the scope
/// registry, the model loop in `analyze`, every emitter's model file
/// list — reads `app.models`. A model that appears later than that is
/// a model half the pipeline cannot see.
///
/// Idempotent, and yields to a real `app/models/active_storage/attachment.rb`
/// if an app ever ships one. Unlike `has_rich_text`'s record, this one
/// does not wait for a declaring macro: upload controllers find
/// Attachment rows by slug / id without the owning model mentioning
/// the macro on every path.
pub fn synthesize_attachment_model(app: &mut App) {
    let class = record_class();
    if app.models.iter().any(|m| m.name == class) {
        return;
    }
    let Some(table) = app.schema.tables.get(&Symbol::from(RECORD_TABLE)) else {
        return;
    };
    let attributes = crate::ingest::model::row_from_table(table);
    // `belongs_to :record, polymorphic: true` — same empty
    // `polymorphic_targets` pattern as `ActionText::RichText`: the
    // owner side is the attached macros' own expansion, not an `as:`
    // declaration in source, so the inverse set stays empty and
    // `attachment.record` stays un-synthesized / gradual. Storage uses
    // the raw `record_id` / `record_type` columns.
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

/// Instance helpers the schema accessors do not cover: reach the blob
/// through `ActiveStorage::Blob.find(@blob_id)` for `url` / `filename`
/// / `content_type`. Called from `model_to_library` for the Attachment
/// model only.
pub(crate) fn push_attachment_record_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    if !is_attachment_model(model) {
        return;
    }
    let blob_ty = Ty::Class {
        id: ClassId(Symbol::from("ActiveStorage::Blob")),
        args: vec![],
    };
    let maybe_blob = Ty::Union {
        variants: vec![blob_ty.clone(), Ty::Nil],
    };
    let filename_ty = Ty::Class {
        id: ClassId(Symbol::from("ActiveStorage::Filename")),
        args: vec![],
    };
    let maybe_filename = Ty::Union {
        variants: vec![filename_ty, Ty::Nil],
    };
    let maybe_str = Ty::Union {
        variants: vec![Ty::Str, Ty::Nil],
    };

    // def blob; ActiveStorage::Blob.find(@blob_id); end
    //
    // Not a belongs_to: Blob is a runtime class. One find keeps url /
    // filename / content_type from each re-querying.
    let blob_local = Symbol::from("b");
    super::model_to_library::push_synth_instance_method(
        methods,
        model,
        Symbol::from("blob"),
        Vec::new(),
        blob_find(ivar("blob_id")),
        Some(fn_sig(vec![], maybe_blob.clone())),
        AccessorKind::Method,
        false,
    );

    // def url; b = blob; b.nil? ? "" : b.redirect_url(""); end
    push(
        methods,
        model,
        Symbol::from("url"),
        seq(vec![
            assign_var(
                &blob_local,
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("blob"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
            ),
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: no_arg_send(var_ref(blob_local.clone()), "nil?"),
                    then_branch: lit_str(""),
                    else_branch: Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(var_ref(blob_local.clone())),
                            method: Symbol::from("redirect_url"),
                            args: vec![lit_str("")],
                            block: None,
                            parenthesized: true,
                        },
                    ),
                },
            ),
        ]),
        Some(fn_sig(vec![], Ty::Str)),
    );

    // def filename; b = blob; b.nil? ? nil : b.filename; end
    push(
        methods,
        model,
        Symbol::from("filename"),
        seq(vec![
            assign_var(
                &blob_local,
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("blob"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
            ),
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: no_arg_send(var_ref(blob_local.clone()), "nil?"),
                    then_branch: nil_lit(),
                    else_branch: no_arg_send(var_ref(blob_local.clone()), "filename"),
                },
            ),
        ]),
        Some(fn_sig(vec![], maybe_filename)),
    );

    // def content_type; b = blob; b.nil? ? nil : b.content_type; end
    push(
        methods,
        model,
        Symbol::from("content_type"),
        seq(vec![
            assign_var(
                &blob_local,
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("blob"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
            ),
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: no_arg_send(var_ref(blob_local.clone()), "nil?"),
                    then_branch: nil_lit(),
                    else_branch: no_arg_send(var_ref(blob_local), "content_type"),
                },
            ),
        ]),
        Some(fn_sig(vec![], maybe_str)),
    );
}

fn push(methods: &mut Vec<MethodDef>, model: &Model, name: Symbol, body: Expr, signature: Option<Ty>) {
    super::model_to_library::push_synth_instance_method(
        methods,
        model,
        name,
        Vec::new(),
        body,
        signature,
        AccessorKind::Method,
        false,
    );
}

fn blob_find(id: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(class_const(&ClassId(Symbol::from("ActiveStorage::Blob")))),
            method: Symbol::from("find"),
            args: vec![id],
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

fn assign_var(name: &Symbol, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: name.clone(),
            },
            value,
        },
    )
}

fn lit_str(v: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Str {
                value: v.to_string(),
            },
        },
    )
}
