//! `has_one_attached :logo` — the reader, the writer, and the after-save
//! that attaches what the writer staged.
//!
//! # What this is and is not
//!
//! Active Storage is three things: attachment ROWS (which record, which
//! blob, under which name), BLOBS (bytes in a service), and VARIANTS
//! (derivatives produced by an image processor). The rows and the blob
//! are modeled by `runtime/ruby/active_storage.rb`; the bytes go
//! through `ActiveStorage::Service`, a per-target seam the ruby family
//! fills with a disk service; variants are IDENTITY (no processor on any
//! target) — see that file's header for the whole split.
//!
//! * `has_one_attached :name` / `has_many_attached :name` are MACROS.
//!   Each expands to a reader whose scope is the three columns Rails
//!   scopes on (`record_id`, `record_type`, `name`), written out rather
//!   than declared for the same reason `has_rich_text`'s is: a bare
//!   `has_one` would find the FIRST attachment on the record regardless
//!   of name, which is right for a model with one attachment and
//!   silently wrong for a model with two. The many form returns an
//!   `ActiveStorage::AttachedMany` proxy whose `attach_blob` APPENDS
//!   (no purge of prior) and whose `attachments` answers an Array of
//!   `ActiveStorage::Attachment` rows.
//! * The `name=` writer stages an ATTACHABLE (an uploaded file, a blob,
//!   a signed id — the runtime's `Blob.from_attachable` narrows it) and
//!   `_save_<name>_attachment`, folded into `after_save`, attaches it
//!   once the record has an id. That is Rails' own order: the blob row
//!   on assignment, the attachment row on save.
//! * `attach(io:, filename:, content_type:)` and `Blob.create_and_upload!
//!   (io:, …)` are grounded to bytes at the call site
//!   (`apply_attach_lowering`) so the runtime never names an IO.
//!
//! # Why the existence half was worth having first
//!
//! `ApplicationHelper.account_logo_body_class` is `"account-has-logo"
//! if Current.account&.logo&.attached?` — it is on the layout, so it
//! runs on EVERY rendered page, and with `logo` undefined every one of
//! those requests raised. Answering just `attached?` was +7 tests and a
//! 7th green file; the writer, the bytes and the identity variant came
//! after, and they are what makes an upload a served image.
//!
//! # The reader never returns nil
//!
//! Rails' `record.logo` hands back an `Attached::One` proxy whether or
//! not anything is attached, so `logo.attached?` is false rather than a
//! NoMethodError on nil. This keeps that contract: the reader always
//! constructs an `ActiveStorage::Attached` (the value type, in
//! `runtime/ruby/active_storage.rb` beside `ActionText::Content` for
//! the same reason — it has no table).

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model, ModelBodyItem};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

fn attached_class() -> ClassId {
    ClassId(Symbol::from("ActiveStorage::Attached"))
}

fn attached_many_class() -> ClassId {
    ClassId(Symbol::from("ActiveStorage::AttachedMany"))
}

/// `has_one_attached :logo` declarations this pass expands, with the
/// span of the declaration. The BLOCK form (`do |attachable|
/// attachable.variant … end`) declares variants, which are not
/// modeled — the attachment half still expands, and the unclaimed
/// diagnostic keeps naming the declaration so the variant gap stays
/// visible.
/// `x.attach(io: F, filename: N, content_type: C)` →
/// `x.attach(F.read, N, C)`.
///
/// Ground the io at the CALL SITE, the same way `signed_id(expires_in:)`
/// and the controller's `expires_in` are grounded. The runtime's
/// `Attached#attach` wants a length it can measure and a name it can
/// store; what Rails hands it is a File, and the shared runtime's RBS
/// has no `File` or `IO` type to name — declaring the parameter
/// `untyped` instead put five new `Ty::Untyped` sites in the runtime and
/// tripped the ceiling gate. Reading the bytes here keeps every
/// parameter a String.
///
/// A literal hash needs `io:` and `filename:`; `content_type:` is
/// optional and grounded through `ActiveStorage.content_type_for_filename`
/// when absent. A Hash that is a *value* (a yielded bag, a local) and
/// the other one-argument attachables (blob, signed id, uploaded file)
/// become `attach_blob(Blob.from_attachable(…))` at the call site.
/// The ruby family unpacks `io:` hashes in `from_attachable`.
pub fn apply_attach_lowering(app: &mut crate::app::App) {
    super::for_each_hook_body(app, &mut rewrite_attach);
    // Test bodies too: every `attach` in the corpus today is written by
    // a test, so gating this to app code would lower nothing at all.
    super::for_each_test_body(app, &mut rewrite_attach);
    // An inline transformation — `attachment.preview(format: :webp,
    // resize_to_limit: [w, h])` in campfire's video presentation and
    // its attachment test — becomes the `Variation` the runtime's
    // `Preview` takes, built here from the same keys a declared
    // `attachable.variant` lowers. Views carry the presentation
    // helper's call, so they are walked too.
    super::for_each_hook_body(app, &mut rewrite_inline_transformation);
    for view in &mut app.views {
        rewrite_inline_transformation(&mut view.body);
    }
    super::for_each_test_body(app, &mut rewrite_inline_transformation);
}

/// `x.preview(format: :webp, resize_to_limit: [w, h])` ->
/// `x.preview(ActiveStorage::Variation.new("", w, h, "webp"))`. Only
/// `preview`: `variant`/`representation` take a declared name in the
/// corpus, and their inline form stays the identity variant the
/// runtime documents. A hash with a key this cannot lower is left as
/// written.
fn rewrite_inline_transformation(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite_inline_transformation);
    let ExprNode::Send { method, args, .. } = &mut *e.node else { return };
    if method.as_str() != "preview" || args.len() != 1 {
        return;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return };
    let Some((width, height, format)) = variation_from_entries(entries) else { return };
    let syn = |node: ExprNode| Expr::new(Span::synthetic(), node);
    let str_lit = |v: &str| syn(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    args[0] = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Const {
            path: vec![Symbol::from("ActiveStorage"), Symbol::from("Variation")],
        })),
        method: Symbol::from("new"),
        args: vec![str_lit(""), width, height, str_lit(&format)],
        block: None,
        parenthesized: true,
    });
}

fn rewrite_attach(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite_attach);
    let ExprNode::Send { recv, method, args, .. } = &mut *e.node else { return };
    // `Blob.create_and_upload!(io:, filename:, content_type:)` is the
    // same three keywords for the same reason (campfire's webhook
    // stores a bot's attachment reply through it), and grounds the
    // same way.
    let name = method.as_str();
    if name != "attach" && name != "create_and_upload!" {
        return;
    }
    if args.len() != 1 {
        return;
    }
    // Only Active Storage receivers — an unrelated `attach` /
    // `create_and_upload!` must not be rewritten.
    let on_attached = name == "attach" && recv_is_attached(recv);
    let on_blob = name == "create_and_upload!" && recv_is_blob_class(recv);
    if !on_attached && !on_blob {
        return;
    }
    if let Some((io, filename, content_type)) = attach_io_hash(&args[0]) {
        let Some(recv_expr) = recv.clone() else { return };
        e.diagnostic = None;
        e.ty = Some(if on_blob { blob_class_ty() } else { Ty::Nil });
        *e.node = grounded_io_call(recv_expr, name, io, filename, content_type);
        return;
    }
    // A Hash bag that is not a literal `io:`/`filename:` shape, or
    // any other one-argument attachable on the proxy: Rails' attach
    // goes through `from_attachable`. `create_and_upload!` of a Hash
    // is that same coercion (it already creates the blob).
    if on_blob {
        if expr_is_hash(&args[0]) {
            e.diagnostic = None;
            *method = Symbol::from("from_attachable");
        }
        return;
    }
    let Some(recv_expr) = recv.clone() else { return };
    // `uploads.attach([file, …])` — Rails' array form. Each element is
    // one attachable; ground them into sequential `attach_blob`
    // / `from_attachable` calls so the Many proxy's APPEND semantics
    // run once per element (has_one still replaces via attach_blob).
    if let ExprNode::Array { elements, .. } = &*args[0].node {
        let steps: Vec<Expr> = elements
            .iter()
            .map(|attachable| {
                let mut step = Expr::new(
                    e.span,
                    attach_blob_from_attachable(recv_expr.clone(), attachable.clone()),
                );
                step.ty = Some(Ty::Nil);
                step
            })
            .collect();
        e.diagnostic = None;
        e.ty = Some(Ty::Nil);
        *e.node = ExprNode::Seq { exprs: steps };
        return;
    }
    let attachable = args[0].clone();
    // Analyze stamped `SendDispatchFailed` on the one-arg Hash form
    // against the three-String `attach` signature; the rewrite is the
    // supported shape, so drop that annotation with the call.
    e.diagnostic = None;
    e.ty = Some(Ty::Nil);
    *e.node = attach_blob_from_attachable(recv_expr, attachable);
}

/// `recv.attach(io:, filename:, content_type?)` with hash values
/// evaluated in Ruby source order (`io`, then `filename`, then
/// `content_type`) before byte grounding, and a nil `content_type`
/// falling back to `content_type_for_filename`.
fn grounded_io_call(
    recv: Expr,
    method: &str,
    io: Expr,
    filename: Expr,
    content_type: Option<Expr>,
) -> ExprNode {
    let span = recv.span;
    let syn = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(span, node);
        e.ty = Some(ty);
        e
    };
    // Underscored so a caller's locals are not shadowed. Bind the
    // IO-side value first: Ruby evaluates hash values in source order,
    // and `.read` / `File.binread` must not run if a later keyword
    // expression raises.
    let io_name = Symbol::from("_attach_io");
    let fn_name = Symbol::from("_attach_filename");
    let ct_name = Symbol::from("_attach_content_type");
    let (io_bind, mut data) = bind_then_ground_io(io, io_name.clone(), span);
    data.ty = Some(Ty::Str);
    let fn_var = || syn(
        ExprNode::Var { id: crate::ident::VarId(0), name: fn_name.clone() },
        Ty::Str,
    );
    let ct_var = || syn(
        ExprNode::Var { id: crate::ident::VarId(0), name: ct_name.clone() },
        Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
    );
    let initial_ct = content_type.unwrap_or_else(|| syn(ExprNode::Lit { value: Literal::Nil }, Ty::Nil));
    let ret_ty = if method == "create_and_upload!" { blob_class_ty() } else { Ty::Nil };
    ExprNode::Seq {
        exprs: vec![
            syn(
                ExprNode::Assign {
                    target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: io_name },
                    value: io_bind,
                },
                Ty::Str,
            ),
            syn(
                ExprNode::Assign {
                    target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: fn_name.clone() },
                    value: filename,
                },
                Ty::Str,
            ),
            syn(
                ExprNode::Assign {
                    target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: ct_name.clone() },
                    value: initial_ct,
                },
                Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            ),
            syn(
                ExprNode::If {
                    cond: syn(
                        ExprNode::Send {
                            recv: Some(ct_var()),
                            method: Symbol::from("nil?"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                        Ty::Bool,
                    ),
                    then_branch: syn(
                        ExprNode::Assign {
                            target: crate::expr::LValue::Var {
                                id: crate::ident::VarId(0),
                                name: ct_name.clone(),
                            },
                            value: content_type_from_filename_expr(fn_var()),
                        },
                        Ty::Str,
                    ),
                    else_branch: syn(ExprNode::Lit { value: Literal::Nil }, Ty::Nil),
                },
                Ty::Nil,
            ),
            syn(
                ExprNode::Send {
                    recv: Some(recv),
                    method: Symbol::from(method),
                    args: vec![data, fn_var(), ct_var()],
                    block: None,
                    parenthesized: true,
                },
                ret_ty,
            ),
        ],
    }
}

fn attach_blob_from_attachable(recv: Expr, attachable: Expr) -> ExprNode {
    let span = recv.span;
    let syn = |node: ExprNode| {
        let mut e = Expr::new(span, node);
        e.ty = Some(Ty::Nil);
        e
    };
    let typed = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(span, node);
        e.ty = Some(ty);
        e
    };
    // Underscored so a caller's locals are not shadowed. Bind the
    // receiver first: Ruby evaluates `recv.attach(x)` left-to-right,
    // and coercing before `recv` would upload an orphaned blob if
    // `recv` then raised. Preserve One vs Many typing from the call
    // site when present.
    let recv_name = Symbol::from("_attach_recv");
    let blob = Symbol::from("_attachable_blob");
    let blob_ty = Ty::Union {
        variants: vec![blob_class_ty(), Ty::Nil],
    };
    let recv_ty = recv.ty.clone().unwrap_or_else(|| Ty::Class {
        id: ClassId(Symbol::from("ActiveStorage::Attached")),
        args: vec![],
    });
    let recv_var = || typed(
        ExprNode::Var { id: crate::ident::VarId(0), name: recv_name.clone() },
        recv_ty.clone(),
    );
    let var = || typed(
        ExprNode::Var { id: crate::ident::VarId(0), name: blob.clone() },
        blob_ty.clone(),
    );
    let coerce = typed(
        ExprNode::Send {
            recv: Some(typed(
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveStorage"), Symbol::from("Blob")],
                },
                blob_class_ty(),
            )),
            method: Symbol::from("from_attachable"),
            args: vec![attachable],
            block: None,
            parenthesized: true,
        },
        blob_ty.clone(),
    );
    ExprNode::Seq {
        exprs: vec![
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: recv_name.clone() },
                value: recv,
            }),
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: blob.clone() },
                value: coerce,
            }),
            typed(
                ExprNode::If {
                    cond: typed(
                        ExprNode::Send {
                            recv: Some(var()),
                            method: Symbol::from("nil?"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                        Ty::Bool,
                    ),
                    then_branch: syn(ExprNode::Lit { value: Literal::Nil }),
                    else_branch: syn(ExprNode::Send {
                        recv: Some(recv_var()),
                        method: Symbol::from("attach_blob"),
                        args: vec![var()],
                        block: None,
                        parenthesized: true,
                    }),
                },
                Ty::Nil,
            ),
        ],
    }
}

fn blob_class_ty() -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from("ActiveStorage::Blob")),
        args: vec![],
    }
}

fn attach_io_hash(arg: &Expr) -> Option<(Expr, Expr, Option<Expr>)> {
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let pick = |name: &str| -> Option<Expr> {
        entries
            .iter()
            .find(|(k, _)| {
                matches!(&*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name)
                    || matches!(&*k.node,
                    ExprNode::Lit { value: Literal::Str { value } } if value == name)
            })
            .map(|(_, v)| v.clone())
    };
    // `io:` and `filename:` required. Extra keys (`identify:`, `key:`)
    // are not reproduced here — those hashes take the attachable path
    // so a dropped option is not silent.
    for (k, _) in entries {
        let known = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                matches!(value.as_str(), "io" | "filename" | "content_type")
            }
            ExprNode::Lit { value: Literal::Str { value } } => {
                matches!(value.as_str(), "io" | "filename" | "content_type")
            }
            _ => false,
        };
        if !known {
            return None;
        }
    }
    let (Some(io), Some(filename)) = (pick("io"), pick("filename")) else {
        return None;
    };
    Some((io, filename, pick("content_type")))
}

/// Bind value evaluated first, then the attach-arg expression that
/// turns the bound local into bytes after `filename` / `content_type`
/// have run. Recognized wrappers keep the Spinel-friendly grounding
/// (`StringIO.new` → payload, `path.open` → `File.binread`) without
/// re-evaluating the payload.
fn bind_then_ground_io(io: Expr, io_name: Symbol, span: Span) -> (Expr, Expr) {
    let io_var = || {
        let mut e = Expr::new(
            span,
            ExprNode::Var { id: crate::ident::VarId(0), name: io_name.clone() },
        );
        e.ty = Some(Ty::Str);
        e
    };
    // `StringIO.new(bytes).read` is `bytes`: bind the payload once and
    // pass the bound local through. The wrapper exists only to give
    // Rails an io; keeping `StringIO` off every target's plate.
    if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*io.node {
        if method.as_str() == "new"
            && args.len() == 1
            && matches!(&*recv.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "StringIO")
        {
            return (args[0].clone(), io_var());
        }
    }
    // `file_fixture("moon.jpg").open` → bind the path, binread later.
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*io.node {
        if method.as_str() == "open" && args.is_empty() {
            let data = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("File")] })),
                    method: Symbol::from("binread"),
                    args: vec![Expr::new(
                        span,
                        ExprNode::Send {
                            recv: Some(io_var()),
                            method: Symbol::from("to_s"),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        },
                    )],
                    block: None,
                    parenthesized: true,
                },
            );
            return (recv.clone(), data);
        }
    }
    let data = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(io_var()),
            method: Symbol::from("read"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    (io, data)
}

fn content_type_from_filename_expr(filename: Expr) -> Expr {
    let span = filename.span;
    let mut expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("ActiveStorage")] },
            )),
            method: Symbol::from("content_type_for_filename"),
            args: vec![filename],
            block: None,
            parenthesized: true,
        },
    );
    expr.ty = Some(Ty::Str);
    expr
}

fn recv_is_attached(recv: &Option<Expr>) -> bool {
    recv.as_ref().and_then(|r| r.ty.as_ref()).is_some_and(ty_is_attached)
}

fn recv_is_blob_class(recv: &Option<Expr>) -> bool {
    let Some(r) = recv else { return false };
    if let ExprNode::Const { path } = &*r.node {
        if path.last().is_some_and(|s| s.as_str() == "Blob") {
            return true;
        }
    }
    r.ty.as_ref().is_some_and(ty_is_blob_class)
}

fn ty_is_attached(ty: &Ty) -> bool {
    match ty {
        Ty::Class { id, .. } => {
            let name = id.0.as_str();
            name == "ActiveStorage::Attached"
                || name == "Attached"
                || name == "ActiveStorage::AttachedMany"
                || name == "AttachedMany"
        }
        Ty::Union { variants } => variants.iter().any(ty_is_attached),
        _ => false,
    }
}

fn ty_is_blob_class(ty: &Ty) -> bool {
    match ty {
        Ty::Class { id, .. } => {
            let name = id.0.as_str();
            name == "ActiveStorage::Blob" || name == "Blob"
        }
        Ty::Union { variants } => variants.iter().any(ty_is_blob_class),
        _ => false,
    }
}

fn expr_is_hash(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Hash { .. }) || e.ty.as_ref().is_some_and(ty_is_hash)
}

fn ty_is_hash(ty: &Ty) -> bool {
    match ty {
        Ty::Hash { .. } => true,
        Ty::Union { variants } => variants.iter().any(ty_is_hash),
        _ => false,
    }
}

pub fn attached_attrs(model: &Model) -> Vec<(Span, Symbol)> {
    macro_attrs(model, "has_one_attached")
}

/// `has_many_attached :uploads` declarations this pass expands.
pub fn many_attached_attrs(model: &Model) -> Vec<(Span, Symbol)> {
    macro_attrs(model, "has_many_attached")
}

fn macro_attrs(model: &Model, macro_name: &str) -> Vec<(Span, Symbol)> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        // `has_*_attached :cover, dependent: :purge_later` — the
        // options hash rides after the name; the reader it declares is
        // the same.
        if method.as_str() != macro_name || args.is_empty() {
            continue;
        }
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
            out.push((expr.span, Symbol::from(value.as_str())));
        }
    }
    out
}

/// One `attachable.variant :name, resize_to_limit: [w, h], format: :f`
/// declaration from a `has_one_attached` block — Rails'
/// `ActiveStorage::Variation`, narrowed to what lowers. The
/// dimensions are EXPRESSIONS, not numbers: campfire writes
/// `resize_to_limit: [ THUMBNAIL_MAX_WIDTH, THUMBNAIL_MAX_HEIGHT ]`,
/// constants of the concern the block sits in, and inside the model
/// (which includes that concern) the bare names resolve exactly as
/// they do in the source. `format` is the symbol's spelling, or ""
/// for "keep the original's".
#[derive(Debug, Clone)]
pub struct VariationDecl {
    pub name: String,
    pub width: Expr,
    pub height: Expr,
    pub format: String,
}

/// The variants a `has_one_attached :<attr>` block declares, in
/// source order; empty for the blockless form. Only `resize_to_limit`
/// and `resize_to_fit` lower (both are libvips' `thumbnail` with the
/// image kept inside the box; the runtime never enlarges). A variant
/// carrying any other transformation — `resize_to_fill` crops,
/// `rotate`, `saver:` options — is left OUT rather than approximated:
/// a thumbnail that is the wrong shape is worse than the
/// `ArgumentError` the runtime raises for a name it was never given,
/// which names the declaration.
pub fn attached_variations(model: &Model, attr: &Symbol) -> Vec<VariationDecl> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node
        else {
            continue;
        };
        if method.as_str() != "has_one_attached" || args.is_empty() {
            continue;
        }
        let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node else { continue };
        if value.as_str() != attr.as_str() {
            continue;
        }
        let ExprNode::Lambda { extra_params, params, body, .. } = &*block.node else { continue };
        if !extra_params.is_empty() {
            continue;
        }
        let Some(attachable) = params.first() else { continue };
        let stmts: Vec<&Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![body],
        };
        for stmt in stmts {
            if let Some(decl) = variation_decl(stmt, attachable) {
                out.push(decl);
            }
        }
    }
    out
}

fn variation_decl(stmt: &Expr, attachable: &Symbol) -> Option<VariationDecl> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*stmt.node else { return None };
    let ExprNode::Var { name, .. } = &*recv.node else { return None };
    if name != attachable || method.as_str() != "variant" || args.len() != 2 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Sym { value: vname } } = &*args[0].node else {
        return None;
    };
    let ExprNode::Hash { entries, .. } = &*args[1].node else { return None };
    let (width, height, format) = variation_from_entries(entries)?;
    Some(VariationDecl { name: vname.as_str().to_string(), width, height, format })
}

/// The `(width, height, format)` a transformation hash names —
/// `resize_to_limit`/`resize_to_fit: [w, h]` and `format: :f`, each
/// optional (0 / "" when absent, the runtime's "no resize" / "keep the
/// original's"). Any other key means the hash is not one this lowers.
fn variation_from_entries(entries: &[(Expr, Expr)]) -> Option<(Expr, Expr, String)> {
    let mut width = None;
    let mut height = None;
    let mut format = String::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match key.as_str() {
            "resize_to_limit" | "resize_to_fit" => {
                let ExprNode::Array { elements, .. } = &*v.node else { return None };
                if elements.len() != 2 {
                    return None;
                }
                width = Some(elements[0].clone());
                height = Some(elements[1].clone());
            }
            "format" => {
                let ExprNode::Lit { value: Literal::Sym { value: f } } = &*v.node else {
                    return None;
                };
                format = f.as_str().to_string();
            }
            _ => return None,
        }
    }
    let int = |n: i64| Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: n } });
    Some((width.unwrap_or_else(|| int(0)), height.unwrap_or_else(|| int(0)), format))
}

/// `[ActiveStorage::Variation.new("thumb", W, H, "png"), …]` — the
/// reader's fourth constructor argument.
fn variations_expr(model: &Model, attr: &Symbol) -> Expr {
    let syn = |node: ExprNode| Expr::new(Span::synthetic(), node);
    let str_lit = |v: &str| syn(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let elements = attached_variations(model, attr)
        .into_iter()
        .map(|d| {
            syn(ExprNode::Send {
                recv: Some(syn(ExprNode::Const {
                    path: vec![Symbol::from("ActiveStorage"), Symbol::from("Variation")],
                })),
                method: Symbol::from("new"),
                args: vec![str_lit(&d.name), d.width, d.height, str_lit(&d.format)],
                block: None,
                parenthesized: true,
            })
        })
        .collect();
    syn(ExprNode::Array { elements, style: Default::default() })
}

/// The same list as Ruby source, for the ruby emitter's batch-loader
/// template (`_preload_batch_<attr>_attachment` constructs the proxy
/// too). The dimension expressions are rendered through the ruby
/// emitter so a constant reference spells as it did in the source.
pub fn variations_ruby_source(model: &Model, attr: &Symbol) -> String {
    let items: Vec<String> = attached_variations(model, attr)
        .iter()
        .map(|d| {
            format!(
                "ActiveStorage::Variation.new({:?}, {}, {}, {:?})",
                d.name,
                crate::emit::ruby::emit_expr(&d.width),
                crate::emit::ruby::emit_expr(&d.height),
                d.format
            )
        })
        .collect();
    format!("[{}]", items.join(", "))
}


/// Synthesize each `has_one_attached` / `has_many_attached` reader onto
/// the declaring model.
pub(crate) fn push_attached_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for (span, attr) in attached_attrs(model) {
        let before = methods.len();
        push_reader(methods, model, &attr);
        for m in &mut methods[before..] {
            m.body.inherit_span(span);
        }
    }
    for (span, attr) in many_attached_attrs(model) {
        let before = methods.len();
        push_many_reader(methods, model, &attr);
        for m in &mut methods[before..] {
            m.body.inherit_span(span);
        }
    }
}

/// ```ruby
/// def logo
///   cached = @logo_cache
///   return cached unless cached.nil?
///   fresh = ActiveStorage::Attached.new("Account", @id, "logo")
///   @logo_cache = fresh
///   fresh
/// end
///
/// def _preload_logo_attachment(att)   # the batch loader's setter
///   @logo_cache = att
///   nil
/// end
/// ```
///
/// ONE proxy per record, as in Rails: `record.logo` hands back the same
/// `Attached::One` on every read, and the proxy remembers its
/// attachment row until `reload`. The proxy used to be constructed
/// fresh on every call, and it queries at ask time, so two reads of
/// `message.attachment.attached?` in one render were two round trips —
/// campfire's `Message#content_type` asks twice per message, and the
/// room page paid 80 attachment lookups for 40 messages. The memo makes
/// it one per record, and `_preload_<attr>_attachment` (which
/// `with_attached_<attr>` drives through the batch loader) makes it
/// one per PAGE.
///
/// A plain constructor, NOT a folded `ActiveStorage::Attachment
/// .where(...).exists?` chain: the query specializer inlines such a
/// chain into a multi-statement SQL block (`stmt = Db.prepare(…);
/// results = []; while Db.step?(stmt) …`), which cannot sit in an
/// argument position — the emit was syntactically invalid and took the
/// whole suite from 47 passing tests to 0. The query belongs in the
/// value object.
///
/// The cache ivar is nilable and never seeded, so a strict target types
/// it `Attached?`; the reader answers the NON-nil local it just checked
/// or built, which keeps the never-nil contract in the signature.
fn push_reader(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    if super::model_to_library::model_defines_instance_method(model, attr)
        || methods
            .iter()
            .any(|m| m.name == *attr && m.receiver == MethodReceiver::Instance)
    {
        return;
    }
    let syn = |node: ExprNode| Expr::new(Span::synthetic(), node);
    let str_lit = |v: &str| syn(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let cache = Symbol::from(format!("{}_cache", attr.as_str()));
    let cached = Symbol::from("cached");
    let fresh = Symbol::from("fresh");
    let var = |name: &Symbol| syn(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() });
    let construct = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Const {
            path: attached_class().0.as_str().split("::").map(Symbol::from).collect(),
        })),
        method: Symbol::from("new"),
        args: vec![
            str_lit(model.name.0.as_str()),
            syn(ExprNode::Ivar { name: Symbol::from("id") }),
            str_lit(attr.as_str()),
            variations_expr(model, attr),
        ],
        block: None,
        parenthesized: true,
    });
    let body = syn(ExprNode::Seq {
        exprs: vec![
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: cached.clone() },
                value: syn(ExprNode::Ivar { name: cache.clone() }),
            }),
            syn(ExprNode::If {
                cond: syn(ExprNode::Send {
                    recv: Some(syn(ExprNode::Send {
                        recv: Some(var(&cached)),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    })),
                    method: Symbol::from("!"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                }),
                then_branch: syn(ExprNode::Return { value: var(&cached) }),
                else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
            }),
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: fresh.clone() },
                value: construct,
            }),
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: cache.clone() },
                value: var(&fresh),
            }),
            var(&fresh),
        ],
    });
    let attached_ty = Ty::Class { id: attached_class(), args: vec![] };
    methods.push(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: attr.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(super::model_to_library::fn_sig(vec![], attached_ty.clone())),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });

    push_writer_and_save(methods, model, attr);

    // The batch loader's setter: `with_attached_<attr>` preloads one
    // proxy per record, row already known, and installs it here.
    let att = Symbol::from("att");
    methods.push(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: preload_setter_name(attr),
        receiver: MethodReceiver::Instance,
        params: vec![crate::dialect::Param::positional(att.clone())],
        body: syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: cache },
                    value: var(&att),
                }),
                syn(ExprNode::Lit { value: Literal::Nil }),
            ],
        }),
        signature: Some(super::model_to_library::fn_sig(vec![(att, attached_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
}

/// ```ruby
/// def avatar=(value)
///   @avatar_pending = ActiveStorage::Blob.from_attachable(value)
///   nil
/// end
///
/// def _save_avatar_attachment       # folded into after_save
///   pending = @avatar_pending
///   if pending.nil?
///     nil
///   else
///     @avatar_pending = nil
///     avatar.attach_blob(pending)
///   end
/// end
/// ```
///
/// Rails' `record.avatar = file` (and so `create!(avatar: file)`,
/// `update!(avatar: file)`, and a permitted `:avatar` param) is an
/// attachable the model holds until it is saved: the blob is created
/// on assignment, the attachment ROW only when the record has an id to
/// point it at — a create assigns before the insert. Same shape as
/// `has_rich_text`'s `_save_rich_text_<attr>`, for the same reason.
///
/// `value` is `untyped` on purpose: what arrives is whatever the
/// params tree or the caller held — an `UploadedFile`, a `Blob`, a
/// signed id String, the bare filename a non-multipart form posts —
/// and `Blob.from_attachable` (ruby-family runtime) is the one place
/// that narrows it by class. A writer typed to one of those would make
/// the honest caller the illegal one.
fn push_writer_and_save(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    let syn = |node: ExprNode| Expr::new(Span::synthetic(), node);
    let pending = Symbol::from(format!("{}_pending", attr.as_str()));
    let value = Symbol::from("value");
    let local = Symbol::from("pending");
    let var = |name: &Symbol| syn(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() });
    let ivar = |name: &Symbol| syn(ExprNode::Ivar { name: name.clone() });
    let nil_lit = || syn(ExprNode::Lit { value: Literal::Nil });

    let coerce = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Const {
            path: vec![Symbol::from("ActiveStorage"), Symbol::from("Blob")],
        })),
        method: Symbol::from("from_attachable"),
        args: vec![var(&value)],
        block: None,
        parenthesized: true,
    });
    super::model_to_library::push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}=", attr.as_str())),
        vec![crate::dialect::Param::positional(value.clone())],
        syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: pending.clone() },
                    value: coerce,
                }),
                nil_lit(),
            ],
        }),
        Some(super::model_to_library::fn_sig(vec![(value, Ty::Untyped)], Ty::Nil)),
        AccessorKind::Method,
        true,
    );

    let saver = Symbol::from(format!("_save_{}", attachment_assoc_name(attr).as_str()));
    let attach = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Send {
            recv: None,
            method: attr.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        })),
        method: Symbol::from("attach_blob"),
        args: vec![var(&local)],
        block: None,
        parenthesized: true,
    });
    super::model_to_library::push_synth_instance_method(
        methods,
        model,
        saver.clone(),
        Vec::new(),
        syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Assign {
                    target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: local.clone() },
                    value: ivar(&pending),
                }),
                syn(ExprNode::If {
                    cond: syn(ExprNode::Send {
                        recv: Some(var(&local)),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    }),
                    then_branch: nil_lit(),
                    else_branch: syn(ExprNode::Seq {
                        exprs: vec![
                            syn(ExprNode::Assign {
                                target: crate::expr::LValue::Ivar { name: pending.clone() },
                                value: nil_lit(),
                            }),
                            attach,
                        ],
                    }),
                }),
            ],
        }),
        Some(super::model_to_library::fn_sig(vec![], Ty::Nil)),
        AccessorKind::Method,
        true,
    );
    super::model_to_library::markers::fold_into_or_push(
        methods,
        model,
        "after_save",
        syn(ExprNode::Send {
            recv: None,
            method: saver,
            args: vec![],
            block: None,
            parenthesized: false,
        }),
    );
}

/// Rails' name for the attachment association behind `has_one_attached
/// :<attr>`: `<attr>_attachment`. It is the spec `with_attached_<attr>`
/// preloads and the arm `_preload_dispatch` answers, so an app that
/// writes Rails' own `includes(logo_attachment: :blob)` lands on the
/// same loader.
pub fn attachment_assoc_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("{}_attachment", attr.as_str()))
}

/// Rails' name behind `has_many_attached :<attr>`: `<attr>_attachments`.
pub fn many_attachments_assoc_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("{}_attachments", attr.as_str()))
}

/// `_preload_<attr>_attachment` — the setter the batch loader calls.
pub fn preload_setter_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_preload_{}", attachment_assoc_name(attr).as_str()))
}

/// `_preload_<attr>_attachments` — Many form of the batch-loader setter.
pub fn many_preload_setter_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_preload_{}", many_attachments_assoc_name(attr).as_str()))
}

/// ```ruby
/// def uploads
///   cached = @uploads_cache
///   return cached unless cached.nil?
///   fresh = ActiveStorage::AttachedMany.new("Doc", @id, "uploads")
///   @uploads_cache = fresh
///   fresh
/// end
///
/// def _preload_uploads_attachments(att)
///   @uploads_cache = att
///   nil
/// end
/// ```
///
/// Always a non-nil Many proxy — same never-nil contract as
/// `has_one_attached`, so `uploads.attach` / `uploads.attachments.last`
/// do not need a nil guard.
fn push_many_reader(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    if super::model_to_library::model_defines_instance_method(model, attr)
        || methods
            .iter()
            .any(|m| m.name == *attr && m.receiver == MethodReceiver::Instance)
    {
        return;
    }
    let syn = |node: ExprNode| Expr::new(Span::synthetic(), node);
    let str_lit = |v: &str| syn(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let cache = Symbol::from(format!("{}_cache", attr.as_str()));
    let cached = Symbol::from("cached");
    let fresh = Symbol::from("fresh");
    let var = |name: &Symbol| syn(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() });
    let construct = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Const {
            path: attached_many_class().0.as_str().split("::").map(Symbol::from).collect(),
        })),
        method: Symbol::from("new"),
        args: vec![
            str_lit(model.name.0.as_str()),
            syn(ExprNode::Ivar { name: Symbol::from("id") }),
            str_lit(attr.as_str()),
        ],
        block: None,
        parenthesized: true,
    });
    let body = syn(ExprNode::Seq {
        exprs: vec![
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: cached.clone() },
                value: syn(ExprNode::Ivar { name: cache.clone() }),
            }),
            syn(ExprNode::If {
                cond: syn(ExprNode::Send {
                    recv: Some(syn(ExprNode::Send {
                        recv: Some(var(&cached)),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    })),
                    method: Symbol::from("!"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                }),
                then_branch: syn(ExprNode::Return { value: var(&cached) }),
                else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
            }),
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: fresh.clone() },
                value: construct,
            }),
            syn(ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: cache.clone() },
                value: var(&fresh),
            }),
            var(&fresh),
        ],
    });
    let many_ty = Ty::Class { id: attached_many_class(), args: vec![] };
    methods.push(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: attr.clone(),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(super::model_to_library::fn_sig(vec![], many_ty.clone())),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });

    let att = Symbol::from("att");
    let var_att = |name: &Symbol| syn(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() });
    methods.push(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: many_preload_setter_name(attr),
        receiver: MethodReceiver::Instance,
        params: vec![crate::dialect::Param::positional(att.clone())],
        body: syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: cache },
                    value: var_att(&att),
                }),
                syn(ExprNode::Lit { value: Literal::Nil }),
            ],
        }),
        signature: Some(super::model_to_library::fn_sig(vec![(att, many_ty)], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
}

// ── the preload scope ──────────────────────────────────────────

/// The scope Rails' macro declares beside the attachment:
/// `with_attached_<attr>`.
///
/// Shared by the analyzer (which registers it as a relation-returning
/// class method so call sites type) and the ruby emit seam (which gives
/// it a body), so the two cannot disagree about which names exist —
/// the same split `rich_text::preload_scope_names` uses, and for the
/// same reason.
pub fn preload_scope_names(model: &Model) -> Vec<Symbol> {
    preload_scopes(model).into_iter().map(|(scope, _assoc)| scope).collect()
}

/// Each preload scope with the association it preloads:
/// `(with_attached_logo, logo_attachment)` /
/// `(with_attached_uploads, uploads_attachments)`. The emitter's
/// relation delegate and the class-side body both read this, so they
/// cannot disagree about what the scope does.
pub fn preload_scopes(model: &Model) -> Vec<(Symbol, Symbol)> {
    let mut out: Vec<(Symbol, Symbol)> = attached_attrs(model)
        .into_iter()
        .map(|(_span, attr)| {
            (
                Symbol::from(format!("with_attached_{}", attr.as_str())),
                attachment_assoc_name(&attr),
            )
        })
        .collect();
    out.extend(many_attached_attrs(model).into_iter().map(|(_span, attr)| {
        (
            Symbol::from(format!("with_attached_{}", attr.as_str())),
            many_attachments_assoc_name(&attr),
        )
    }));
    out
}

/// Give the preload scope a body: `def self.with_attached_avatar(__rel
/// = ActiveRecord::Relation.new(self)) = __rel.preload(:avatar_attachment)`.
///
/// In Rails this is `includes(<attr>_attachment: :blob)` — a query-plan
/// hint that changes how many round trips a page costs and nothing
/// about which rows come back. It used to be IDENTITY here, because the
/// reader queried per record at ask time and the hint had nothing to
/// attach to; that was an N+1 stated rather than hidden, and on
/// campfire's room page it was 80 of 233 round trips. The reader
/// memoizes now (`push_reader`) and `_preload_dispatch` carries an arm
/// for `<attr>_attachment` (`emit::ruby::library::preload_targets`),
/// so the relation's `to_a` batches every record's attachment row into
/// one query and installs a row-bearing proxy on each record. Leaving
/// the method undefined was never an option: campfire's
/// `Message.with_attached_attachment` is inside the scope chain
/// `rooms#show` renders from, so a NameError there is every room page.
///
/// Ruby-family only, for the same reason `push_scope_methods` is:
/// `ActiveRecord::Relation` is a CRuby/JRuby runtime class.
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
            params: vec![crate::dialect::Param::with_default(
                rel.clone(),
                super::model_to_library::relation_new_self(),
            )],
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: crate::ident::VarId(0), name: rel.clone() },
                    )),
                    method: Symbol::from("preload"),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: assoc.clone() } },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
            // No signature — see the note on the rich-text twin: a
            // `Ty::Relation` in a signature is a relation REACHING EMIT,
            // which every emitter reports as unsupported. The analyzer's
            // own registration types the CALL SITE, which is the half
            // that matters.
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}
