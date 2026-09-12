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
//! * `has_one_attached :name` is a MACRO. It expands to a reader whose
//!   scope is the three columns Rails scopes on (`record_id`,
//!   `record_type`, `name`), written out rather than declared for the
//!   same reason `has_rich_text`'s is: a bare `has_one` would find the
//!   FIRST attachment on the record regardless of name, which is right
//!   for a model with one attachment and silently wrong for a model
//!   with two.
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
/// Keyword-form only, which is the only form Rails documents for the
/// `io:` variant (`attach(io:, filename:, content_type:)`). The
/// single-argument `attach(uploaded_file)` shape is a DIFFERENT
/// attachable and is left alone rather than guessed at.
pub fn apply_attach_lowering(app: &mut crate::app::App) {
    super::for_each_hook_body(app, &mut rewrite_attach);
    // Test bodies too: every `attach` in the corpus today is written by
    // a test, so gating this to app code would lower nothing at all.
    super::for_each_test_body(app, &mut rewrite_attach);
}

fn rewrite_attach(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite_attach);
    let ExprNode::Send { method, args, .. } = &mut *e.node else { return };
    // `Blob.create_and_upload!(io:, filename:, content_type:)` is the
    // same three keywords for the same reason (campfire's webhook
    // stores a bot's attachment reply through it), and grounds the
    // same way.
    if (method.as_str() != "attach" && method.as_str() != "create_and_upload!") || args.len() != 1
    {
        return;
    }
    let ExprNode::Hash { entries, kwargs: true } = &*args[0].node else { return };
    let pick = |name: &str| -> Option<Expr> {
        entries
            .iter()
            .find(|(k, _)| {
                matches!(&*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name)
            })
            .map(|(_, v)| v.clone())
    };
    // All three, and nothing else: an attach carrying an option this
    // does not reproduce is left to fail by name rather than silently
    // dropped.
    let (Some(io), Some(filename), Some(content_type)) =
        (pick("io"), pick("filename"), pick("content_type"))
    else {
        return;
    };
    if entries.len() != 3 {
        return;
    }
    let span = io.span;
    // `StringIO.new(bytes).read` is `bytes`: the wrapper exists only to
    // give Rails an io, and reading it back through StringIO would put
    // that class on every target's plate for nothing.
    let unwrapped = match &*io.node {
        ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "new"
                && args.len() == 1
                && matches!(&*recv.node, ExprNode::Const { path }
                    if path.len() == 1 && path[0].as_str() == "StringIO") =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    // `file_fixture("moon.jpg").open` is a Pathname opened for reading;
    // `File.binread(path.to_s)` is the same bytes without an IO in the
    // middle (spinel's blockless `Pathname#open` has no value to read
    // from — it yields).
    let unwrapped = unwrapped.or_else(|| match &*io.node {
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "open" && args.is_empty() =>
        {
            Some(Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("File")] })),
                    method: Symbol::from("binread"),
                    args: vec![Expr::new(
                        span,
                        ExprNode::Send {
                            recv: Some(recv.clone()),
                            method: Symbol::from("to_s"),
                            args: Vec::new(),
                            block: None,
                            parenthesized: false,
                        },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    });
    let mut data = match unwrapped {
        Some(bytes) => bytes,
        None => Expr::new(
            span,
            ExprNode::Send {
                recv: Some(io),
                method: Symbol::from("read"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        ),
    };
    data.ty = Some(Ty::Str);
    *args = vec![data, filename, content_type];
}

pub fn attached_attrs(model: &Model) -> Vec<(Span, Symbol)> {
    let mut out = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "has_one_attached" || args.len() != 1 {
            continue;
        }
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
            out.push((expr.span, Symbol::from(value.as_str())));
        }
    }
    out
}


/// Synthesize each `has_one_attached` reader onto the declaring model.
pub(crate) fn push_attached_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for (span, attr) in attached_attrs(model) {
        let before = methods.len();
        push_reader(methods, model, &attr);
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

/// `_preload_<attr>_attachment` — the setter the batch loader calls.
pub fn preload_setter_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_preload_{}", attachment_assoc_name(attr).as_str()))
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
/// `(with_attached_logo, logo_attachment)`. The emitter's relation
/// delegate and the class-side body both read this, so they cannot
/// disagree about what the scope does.
pub fn preload_scopes(model: &Model) -> Vec<(Symbol, Symbol)> {
    attached_attrs(model)
        .into_iter()
        .map(|(_span, attr)| {
            (
                Symbol::from(format!("with_attached_{}", attr.as_str())),
                attachment_assoc_name(&attr),
            )
        })
        .collect()
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
