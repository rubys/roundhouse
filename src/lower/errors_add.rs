//! `errors.add` grounding: `errors.add(:field, "msg")` →
//! `errors << "Field msg"` and `error_details << ActiveRecord::ErrorDetail.new("field", "msg", "msg", "Field msg", false, true)`,
//! so the accumulator keeps what Rails keeps for each message; `errors.clear`
//! empties both.
//!
//! The shared runtime's error accumulator is a plain `Array[String]` —
//! the validates lowering bakes humanized full messages at lower time
//! ("Short can't be blank") — so hand-written `add` calls ground into
//! the same shape. `:base` contributes the bare message (Rails
//! semantics); a dynamic message interpolates after the humanized
//! field; a missing message defaults to Rails' "is invalid".
//!
//! Any receiver spelling grounds: `errors` as a zero-arg send on
//! `self` (a model adding to its own errors during validation) or on
//! another expression (`record.errors.add(...)` from a controller —
//! lobsters' duplicate-comment guard). The rewrite keeps the receiver,
//! so both land on the same accumulator. A dynamic (non-symbol) field
//! still joins the residue ledger — the accumulator is an
//! `Array[String]`, which has no `add`, so on strict targets each such
//! site is a named per-target gap rather than a silent compile error
//! (`lower_residue` warning, pass `errors_add`). Adjacent
//! string-literal concats in the message (`"a " << "b"`, lobsters'
//! line-wrap idiom) fold to one literal first — a runtime `<<` on a
//! frozen literal is a hazard the bake sidesteps.
//!
//! `errors.full_message(:field, msg)` bakes the same text without touching
//! the accumulator: Rails answers the humanized field before the message
//! (the bare message for `:base`), which is a pure function of the two
//! arguments. A record that is not a plain read is still built before the
//! text, as `User.new && "Email is invalid"`. Only that shape is typed
//! (`full_message_bakes`); any other spelling keeps analyze's Errors gap.
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form. Scope is `for_each_hook_body` (views
//! excluded like every hook pass — the construct has no view presence).

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;

/// Rewrite self-receiver `errors.add` sends across every hook body.
/// Returns the residue ledger: recognizable `errors.add` sites left
/// dynamic, with the reason.
pub fn apply_errors_add_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let data = LabelData::of(app);
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        rewrite_errors_add(body, &data.at(owner), &mut diags)
    });
    if data.used.get() {
        app.library_classes.push(error_tables_class(&data));
    }
    diags
}

/// `ErrorTables`, generated only for an app with a run-time `errors.add`:
/// the tables `ActiveSupport.error_detail` names an attribute by and looks
/// a model's own messages up in, by the record's class name, each a typed
/// `Hash[String, String]` (empty for a model with none).
fn error_tables_class(data: &LabelData) -> crate::dialect::LibraryClass {
    crate::dialect::LibraryClass {
        name: crate::ident::ClassId(Symbol::from("ErrorTables")),
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: vec![table_method("names", &data.names), table_method("overrides", &data.overrides)],
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}

/// `def self.<name>(class_name)`: `if class_name == "<Model>" then {…}
/// elsif … else {} end`, every branch a `Hash[String, String]`.
fn table_method(name: &str, rows: &[(String, Vec<(String, String)>)]) -> crate::dialect::MethodDef {
    use crate::ty::Ty;
    let span = crate::span::Span::synthetic();
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Str) };
    let hash = |entries: &[(String, String)]| {
        let mut h = Expr::new(
            span,
            ExprNode::Hash { entries: entries.iter().map(|(k, v)| (str_lit(span, k), str_lit(span, v))).collect(), kwargs: false },
        );
        h.ty = Some(hash_ty.clone());
        h
    };
    let param = Symbol::from("class_name");
    let mut body = hash(&[]);
    for (class, entries) in rows.iter().rev() {
        let mut read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: param.clone() });
        read.ty = Some(Ty::Str);
        let mut cond = Expr::new(span, ExprNode::Send { recv: Some(read), method: Symbol::from("=="), args: vec![str_lit(span, class)], block: None, parenthesized: false });
        cond.ty = Some(Ty::Bool);
        let mut branch = Expr::new(span, ExprNode::If { cond, then_branch: hash(entries), else_branch: body });
        branch.ty = Some(hash_ty.clone());
        body = branch;
    }
    crate::dialect::MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: span,
        name: Symbol::from(name),
        receiver: crate::dialect::MethodReceiver::Class,
        params: vec![crate::dialect::Param::positional(param)],
        body,
        signature: Some(crate::lower::model_to_library::fn_sig(vec![(Symbol::from("class_name"), Ty::Str)], hash_ty)),
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(Symbol::from("ErrorTables")),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}


/// Each model's stamped i18n, and the app's for an `errors` whose model
/// is not known, held apart from the `App` the walk borrows mutably.
pub(crate) struct LabelData {
    models: std::collections::HashMap<crate::ident::ClassId, crate::i18n::ModelI18n>,
    unknown: crate::i18n::ModelI18n,
    /// The app's whole catalog: the global error messages a run-time
    /// `errors.add` (`dynamic_add`) bakes for its kind.
    catalog: crate::i18n::Catalog,
    /// Each model with translated attribute names, by class name, for
    /// `ErrorTables.names`.
    names: Vec<(String, Vec<(String, String)>)>,
    /// Each model's own messages ("<attr>.<kind>", "<kind>") in Rails'
    /// lookup order, with its scope under "__scope", for
    /// `ErrorTables.overrides`; a model with none and the `activerecord`
    /// scope is left out (its table is empty).
    overrides: Vec<(String, Vec<(String, String)>)>,
    /// Set once a run-time `errors.add` is rewritten: `ErrorTables` is then
    /// generated (`error_tables_class`).
    used: std::cell::Cell<bool>,
}

impl LabelData {
    pub(crate) fn of(app: &App) -> Self {
        LabelData {
            models: app.models.iter().map(|m| (m.name.clone(), m.i18n.clone())).collect(),
            unknown: crate::i18n::ModelI18n {
                scope: "activerecord".into(),
                keys: Vec::new(),
                catalog: app.i18n.subset_for("activerecord", &[]),
            },
            // Rails' English for an app built outside ingest, as the
            // literal path's `ModelI18n` reads it.
            catalog: app.i18n.or_rails_default().clone(),
            names: app
                .models
                .iter()
                .map(|m| {
                    // With the model's human name, for a `%{model}` interpolation.
                    let mut names = m.i18n.translated_attribute_names();
                    names.push(("__model".to_string(), m.i18n.model_human_name()));
                    (m.name.0.as_str().to_string(), names)
                })
                .collect(),
            overrides: app
                .models
                .iter()
                .filter_map(|m| {
                    let mut rows = m.i18n.error_overrides();
                    let scope = m.i18n.error_scope().to_string();
                    if rows.is_empty() && scope == "activerecord" {
                        return None;
                    }
                    rows.push(("__scope".to_string(), scope));
                    Some((m.name.0.as_str().to_string(), rows))
                })
                .collect(),
            used: std::cell::Cell::new(false),
        }
    }

    pub(crate) fn at<'a>(&'a self, owner: Option<&'a crate::ident::ClassId>) -> Labels<'a> {
        Labels { data: self, owner }
    }
}

/// What a baked message needs to name its attribute as Rails would.
pub(crate) struct Labels<'a> {
    data: &'a LabelData,
    owner: Option<&'a crate::ident::ClassId>,
}

impl Labels<'_> {
    /// The model whose `errors` this is: the reader's receiver's type, or the owner for `errors` on self.
    fn model(&self, errors_reader: &Expr) -> &crate::i18n::ModelI18n {
        let ExprNode::Send { recv, .. } = &*errors_reader.node else { return &self.data.unknown };
        let id = match recv.as_ref().map(|r| (&*r.node, r.ty.as_ref())) {
            None | Some((ExprNode::SelfRef, _)) => self.owner,
            Some((_, Some(crate::ty::Ty::Class { id, .. }))) => Some(id),
            _ => None,
        };
        id.and_then(|id| self.data.models.get(id)).unwrap_or(&self.data.unknown)
    }
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "errors_add",
        "errors.add",
        expr.span,
        reason,
        format!(
            "`errors.add` left as dynamic dispatch ({reason}) — the error \
             accumulator is an Array[String] with no `add`; ground by hand \
             or extend the errors_add lowering"
        ),
    )
}

/// Fold `"a" << "b"` / `"a" + "b"` chains of string literals into one
/// literal (left-recursively, so multi-line wraps fold whole). Any
/// non-literal operand leaves the expression untouched.
fn fold_str_concat(e: Expr) -> Expr {
    let ExprNode::Send { recv: Some(l), method, args, block: None, .. } = &*e.node else {
        return e;
    };
    if !(method.as_str() == "<<" || method.as_str() == "+") || args.len() != 1 {
        return e;
    }
    let left = fold_str_concat(l.clone());
    let (ExprNode::Lit { value: Literal::Str { value: lv } },
         ExprNode::Lit { value: Literal::Str { value: rv } }) = (&*left.node, &*args[0].node)
    else {
        return e;
    };
    Expr::new(e.span, ExprNode::Lit { value: Literal::Str { value: format!("{lv}{rv}") } })
}

fn rewrite_errors_add(expr: &mut Expr, labels: &Labels, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_errors_add(c, labels, diags));
    if let ExprNode::Send { recv, method, args, block, .. } = &*expr.node {
        if method.as_str() == "full_message" && full_message_bakes(recv.as_ref(), args, block.as_ref()) {
            let span = expr.span;
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Send { recv, args, .. } = node else { unreachable!() };
            let i18n = labels.model(recv.as_ref().expect("checked errors reader"));
            let mut args = args.into_iter();
            let field_expr = args.next().expect("checked two args");
            let ExprNode::Lit { value: Literal::Sym { value: field } } = &*field_expr.node else {
                unreachable!()
            };
            let mut message = baked_message(i18n, span, field.as_str(), args.next().map(fold_str_concat));
            message.ty = Some(crate::ty::Ty::Str);
            let ExprNode::Send { recv: owner, .. } = *recv.expect("checked errors reader").node else { unreachable!() };
            *expr = match owner {
                // Not a Seq: an emitter hoists one out of `||` and `?:`, which would build the record where Rails does not.
                Some(owner) if !reads_without_effect(&owner) => {
                    let mut built = Expr::new(
                        span,
                        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, surface: crate::expr::BoolOpSurface::Symbol, left: owner, right: message },
                    );
                    built.ty = Some(crate::ty::Ty::Str);
                    built
                }
                _ => message,
            };
            return;
        }
    }
    if rewrite_import(expr, labels, diags) {
        return;
    }
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node {
        if method.as_str() == "clear" && args.is_empty() {
            if let ExprNode::Send { recv: owner, method: em, args: ea, .. } = &*r.node {
                if em.as_str() == "errors" && ea.is_empty() {
                    if !owner_reads_once(owner.as_ref()) {
                        diags.push(residue(expr, "the record is not a plain read, and clearing both arrays reads it twice"));
                        return;
                    }
                    let span = expr.span;
                    let owner = owner.clone();
                    let ty = expr.ty.clone();
                    let clear_errors = std::mem::replace(expr, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
                    let mut clear_details = Expr::new(
                        span,
                        ExprNode::Send { recv: Some(error_details(owner, span)), method: Symbol::from("clear"), args: Vec::new(), block: None, parenthesized: false },
                    );
                    clear_details.ty = Some(error_details_ty());
                    // Details first, so the value is the `errors` array the caller cleared
                    *expr = Expr::new(span, ExprNode::Seq { exprs: vec![clear_details, clear_errors] });
                    expr.ty = ty;
                    return;
                }
            }
        }
    }
    // Any `add` on an `errors` reader is this pass's construct; decide
    // ground-vs-residue below.
    let recognized = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if method.as_str() == "add"
                && matches!(&*r.node, ExprNode::Send { method: em, args: ea, .. }
                    if em.as_str() == "errors" && ea.is_empty())
    );
    if !recognized {
        return;
    }
    let owner_once = match &*expr.node {
        ExprNode::Send { recv: Some(r), .. } => match &*r.node {
            ExprNode::Send { recv: owner, .. } => owner_reads_once(owner.as_ref()),
            _ => false,
        },
        _ => false,
    };
    if !owner_once {
        diags.push(residue(expr, "the record is not a plain read, and pushing the message and its attribute reads it twice"));
        return;
    }
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { args, .. }
            if !args.is_empty()
                && args.len() <= 2
                && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
    );
    if !matches {
        if dynamic_add(expr, labels) {
            return;
        }
        let reason = match &*expr.node {
            ExprNode::Send { args, .. }
                if args.first().is_some_and(
                    |a| !matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { .. } })) =>
            {
                "dynamic field"
            }
            _ => "unrecognized arg shape",
        };
        diags.push(residue(expr, reason));
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, args, .. } = node else { unreachable!() };
    let mut args = args.into_iter();
    let field_expr = args.next().expect("checked non-empty");
    let ExprNode::Lit { value: Literal::Sym { value: field } } = &*field_expr.node else {
        unreachable!()
    };
    let mut msg = args.next().map(fold_str_concat);
    // Rails' options spelling: `errors.add(:field, message: "…")`.
    // The kwargs hash IS the message carrier — unwrap a sole
    // `message:` entry; any other option set stays residue (put the
    // Send back — it was already moved out).
    let sole_message = matches!(
        msg.as_ref().map(|m| &*m.node),
        Some(ExprNode::Hash { entries, .. })
            if entries.len() == 1
                && matches!(&*entries[0].0.node,
                    ExprNode::Lit { value: Literal::Sym { value } }
                        if value.as_str() == "message")
    );
    if sole_message {
        let Some(ExprNode::Hash { entries, .. }) = msg.take().map(|m| *m.node) else {
            unreachable!()
        };
        msg = Some(fold_str_concat(entries.into_iter().next().unwrap().1));
    } else if matches!(msg.as_ref().map(|m| &*m.node), Some(ExprNode::Hash { .. })) {
        *expr.node = ExprNode::Send {
            recv,
            method: Symbol::from("add"),
            args: vec![field_expr, msg.unwrap()],
            block: None,
            parenthesized: true,
        };
        if !dynamic_add(expr, labels) {
            diags.push(residue(expr, "unrecognized arg shape"));
        }
        return;
    }
    let reader = recv.as_ref().expect("checked errors reader");
    let i18n = labels.model(reader);
    let mut hoisted: Option<Expr> = None;
    // Rails looks a Symbol message (or none, `:invalid`) up as an error type.
    let kind = match msg.as_ref().map(|m| &*m.node) {
        None => Some("invalid".to_string()),
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.as_str().to_string()),
        _ => None,
    };
    let ExprNode::Send { recv: reader_owner, .. } = &*reader.node else { unreachable!() };
    let symbolic = kind.is_some();
    let (message, kind, bare) = match kind {
        Some(kind) => (
            parts_expr(span, i18n.full_error(field.as_str(), &kind, &Default::default()), reader_owner.as_ref(), field.as_str()),
            str_lit(span, &kind),
            parts_expr(span, i18n.error(field.as_str(), &kind, &Default::default()), reader_owner.as_ref(), field.as_str()),
        ),
        None => {
            let mut bare = msg.clone().unwrap_or_else(|| unreachable!("a missing message is `:invalid`"));
            // A computed String message (`I18n.t(…)`, an interpolation of
            // calls) is evaluated once into a local, which the record then
            // keeps twice. Any other one stays residue.
            if !reads_without_effect_deep(&bare) && matches!(bare.ty, Some(crate::ty::Ty::Str)) {
                let local = Symbol::from(format!("__error_message_{}", span.start));
                let mut read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: local.clone() });
                read.ty = Some(crate::ty::Ty::Str);
                hoisted = Some(Expr::new(
                    span,
                    ExprNode::Assign { target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: local }, value: bare },
                ));
                bare = read;
                msg = Some(bare.clone());
            }
            if !reads_without_effect_deep(&bare) {
                diags.push(residue(expr, "the message is not a plain read, and the record keeps it twice"));
                *expr.node = ExprNode::Send {
                    recv,
                    method: Symbol::from("add"),
                    args: vec![field_expr, bare],
                    block: None,
                    parenthesized: true,
                };
                return;
            }
            // Rails' type for a String message is the message itself
            (baked_message(i18n, span, field.as_str(), msg), bare.clone(), bare)
        }
    };
    // `<<` returns the accumulator it appends to (Array#<< / the errors
    // collection), so the synthesized send's result type is the
    // receiver's. Stamp it: this pass runs *after* analyze, and the
    // post-lowering `diagnose` walk reads stamped types without re-
    // dispatching — leaving `ty = None` here makes it false-positive a
    // `send_dispatch_failed` on a receiver it can see the type of.
    let recv_ty = recv.as_ref().and_then(|r| r.ty.clone());
    let owner = match recv.as_ref().map(|r| &*r.node) {
        Some(ExprNode::Send { recv: owner, .. }) => owner.clone(),
        _ => unreachable!("checked errors reader"),
    };
    let mut push_message = Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from("<<"), args: vec![message], block: None, parenthesized: false },
    );
    push_message.ty = recv_ty.clone();
    // Options other than `message:` stayed residue above, so nothing sits beside the type.
    let pushed = attributed_push(push_message, owner, field.as_str(), kind, symbolic, true, bare, span);
    *expr = match hoisted {
        Some(assign) => Expr::new(span, ExprNode::Seq { exprs: vec![assign, pushed] }),
        None => pushed,
    };
}

fn is_errors_ty(e: &Expr) -> bool {
    matches!(&e.ty, Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
}

fn is_error_ty(e: &Expr) -> bool {
    matches!(&e.ty, Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Error")
}

/// The attribute `errors.import` files the error under: `None` for the error's own, else the `attribute:` override.
fn import_attribute(args: &[Expr]) -> Option<Option<&Expr>> {
    match args {
        [error] if is_error_ty(error) => Some(None),
        [error, options] if is_error_ty(error) => match &*options.node {
            ExprNode::Hash { entries, .. } => match entries.as_slice() {
                [(key, value)]
                    if matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "attribute")
                        && matches!(value.ty, Some(crate::ty::Ty::Sym)) =>
                {
                    Some(Some(value))
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The record whose errors `errors.merge!` imports: an `errors` read, or a record.
fn merged_owner(args: &[Expr], is_model: &dyn Fn(&crate::ident::ClassId) -> bool) -> Option<Option<Expr>> {
    let [other] = args else { return None };
    match &*other.node {
        ExprNode::Send { recv, method, args, block: None, .. } if method.as_str() == "errors" && args.is_empty() && is_errors_ty(other) => {
            Some(recv.clone())
        }
        _ if matches!(&other.ty, Some(crate::ty::Ty::Class { id, .. }) if is_model(id)) => Some(Some(other.clone())),
        _ => None,
    }
}

/// Shared with analyze, which types `import` and `merge!` only when this holds.
pub(crate) fn import_grounds(method: &str, args: &[Expr], is_model: &dyn Fn(&crate::ident::ClassId) -> bool) -> bool {
    match method {
        "import" => import_attribute(args).is_some(),
        "merge!" => merged_owner(args, is_model).is_some(),
        _ => false,
    }
}

/// `errors.format` around the attribute's name and the message, when it names each once, in that order.
fn format_pieces(format: &str) -> Option<(String, String, String)> {
    let (before, rest) = format.split_once("%{attribute}")?;
    let (between, after) = rest.split_once("%{message}")?;
    (!before.contains("%{") && !between.contains("%{") && !after.contains("%{"))
        .then(|| (before.to_string(), between.to_string(), after.to_string()))
}

/// `<record>.errors.add(attribute, kind_or_message, **options)` whose
/// attribute or options are known only at run time (a validator's
/// `validate_each(record, attribute, value)`) → `ActiveSupport.errors_add(
/// …)`, pushed with its full message onto the record's errors. The kind is a literal Symbol, or a String message stands in
/// for it, so the global messages Rails would fall back to for that kind
/// (`site`) are baked here, and the helper looks the record's own model
/// first. Any other shape (a dynamic kind, a `message:` option, a
/// record read twice) stays residue.
fn dynamic_add(expr: &mut Expr, labels: &Labels) -> bool {
    let ExprNode::Send { recv: Some(reader), args, .. } = &*expr.node else { return false };
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { return false };
    if !owner_reads_once(owner.as_ref()) || args.is_empty() || args.len() > 3 {
        return false;
    }
    let span = expr.span;
    let (kind, message, options) = match &args[1..] {
        [] => (Some("invalid".to_string()), None, None),
        [Expr { node, .. }] if matches!(&**node, ExprNode::Lit { value: Literal::Sym { .. } }) => {
            let ExprNode::Lit { value: Literal::Sym { value } } = &**node else { unreachable!() };
            (Some(value.as_str().to_string()), None, None)
        }
        [opts] if matches!(&*opts.node, ExprNode::Hash { .. }) => (Some("invalid".to_string()), None, Some(opts)),
        // Computed (`"saw " + value.inspect`) too: the helper takes it once.
        [m] if matches!(m.ty, Some(crate::ty::Ty::Str)) => (None, Some(m.clone()), None),
        [k, opts] if matches!(&*opts.node, ExprNode::Hash { .. }) => match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => (Some(value.as_str().to_string()), None, Some(opts)),
            _ => return false,
        },
        _ => return false,
    };
    // Options as a String-keyed Hash; `message:` (Rails' override) isn't taken.
    let mut entries = Vec::new();
    if let Some(opts) = options {
        let ExprNode::Hash { entries: given, .. } = &*opts.node else { unreachable!() };
        for (k, v) in given {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return false };
            if key.as_str() == "message" {
                return false;
            }
            // Interpolated as text, as I18n does; `count:` also picks the plural form.
            let mut text = Expr::new(span, ExprNode::Send { recv: Some(v.clone()), method: Symbol::from("to_s"), args: vec![], block: None, parenthesized: false });
            text.ty = Some(crate::ty::Ty::Str);
            entries.push((str_lit(span, key.as_str()), text));
        }
    }
    let Some((before, between, after)) = format_pieces(labels.model(reader).errors_format()) else { return false };
    let mut site = Vec::new();
    if let Some(kind) = &kind {
        let catalog = &labels.data.catalog;
        let mut put = |name: String, key: String| {
            for (suffix, k) in [("", key.clone()), (".zero", format!("{key}.zero")), (".one", format!("{key}.one")), (".other", format!("{key}.other"))] {
                if let Some(text) = catalog.lookup(&k) {
                    site.push((str_lit(span, &format!("{name}{suffix}")), str_lit(span, text)));
                }
            }
        };
        put("ar_messages".into(), format!("activerecord.errors.messages.{kind}"));
        put("am_messages".into(), format!("activemodel.errors.messages.{kind}"));
        let prefix = "errors.attributes.";
        // An attribute with the kind, plain or only in its plural forms.
        let attrs: std::collections::BTreeSet<String> = catalog
            .entries
            .keys()
            .filter_map(|k| k.strip_prefix(prefix))
            .filter_map(|rest| {
                let rest = [".zero", ".one", ".other"].iter().find_map(|form| rest.strip_suffix(form)).unwrap_or(rest);
                rest.strip_suffix(&format!(".{kind}")).map(str::to_string)
            })
            .filter(|a| !a.contains('.'))
            .collect();
        for attr in attrs {
            put(format!("attributes.{attr}"), format!("{prefix}{attr}.{kind}"));
        }
        put("messages".into(), format!("errors.messages.{kind}"));
    }
    let str_hash = |entries: Vec<(Expr, Expr)>, value: crate::ty::Ty| {
        let mut h = Expr::new(span, ExprNode::Hash { entries, kwargs: false });
        h.ty = Some(crate::ty::Ty::Hash { key: std::sync::Arc::new(crate::ty::Ty::Str), value: std::sync::Arc::new(value) });
        h
    };
    let nil = || {
        let mut n = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
        n.ty = Some(crate::ty::Ty::Nil);
        n
    };
    let mut name = Expr::new(span, ExprNode::Send { recv: Some(args[0].clone()), method: Symbol::from("to_s"), args: vec![], block: None, parenthesized: false });
    name.ty = Some(crate::ty::Ty::Str);
    let call_args = vec![
        name,
        kind.as_deref().map(|k| str_lit(span, k)).unwrap_or_else(nil),
        message.unwrap_or_else(nil),
        str_hash(entries, crate::ty::Ty::Str),
        str_hash(site, crate::ty::Ty::Str),
        table_call(span, "names", owner.clone()),
        table_call(span, "overrides", owner.clone()),
        str_lit(span, &before),
        str_lit(span, &between),
        str_lit(span, &after),
    ];
    let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
    support.ty = Some(crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveSupport")), args: vec![].into() });
    let detail_ty = crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveRecord::ErrorDetail")), args: vec![].into() };
    let mut call = Expr::new(
        span,
        ExprNode::Send { recv: Some(support), method: Symbol::from("error_detail"), args: call_args, block: None, parenthesized: true },
    );
    call.ty = Some(detail_ty.clone());
    // `detail = ActiveSupport.error_detail(…); <owner>.errors << detail.full_message; <owner>.error_details << detail`
    let local = Symbol::from(format!("__error_detail_{}", span.start));
    let read = || {
        let mut r = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: local.clone() });
        r.ty = Some(detail_ty.clone());
        r
    };
    let assign = Expr::new(span, ExprNode::Assign { target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: local.clone() }, value: call });
    let mut full = Expr::new(span, ExprNode::Send { recv: Some(read()), method: Symbol::from("full_message"), args: vec![], block: None, parenthesized: false });
    full.ty = Some(crate::ty::Ty::Str);
    let mut errors = reader.clone();
    errors.ty = Some(crate::ty::Ty::Array { elem: std::sync::Arc::new(crate::ty::Ty::Str) });
    let mut push_message = Expr::new(span, ExprNode::Send { recv: Some(errors), method: Symbol::from("<<"), args: vec![full], block: None, parenthesized: false });
    push_message.ty = Some(crate::ty::Ty::Array { elem: std::sync::Arc::new(crate::ty::Ty::Str) });
    let mut push_detail = Expr::new(span, ExprNode::Send { recv: Some(error_details(owner.clone(), span)), method: Symbol::from("<<"), args: vec![read()], block: None, parenthesized: false });
    push_detail.ty = Some(error_details_ty());
    let mut call = Expr::new(span, ExprNode::Seq { exprs: vec![assign, push_message, push_detail] });
    call.ty = Some(error_details_ty());
    labels.data.used.set(true);
    *expr = call;
    true
}

/// `ErrorTables.<table>(<record>.class.name)`, the record's model's table.
fn table_call(span: crate::span::Span, table: &str, owner: Option<Expr>) -> Expr {
    let record = owner.unwrap_or_else(|| Expr::new(span, ExprNode::SelfRef));
    // Stamped: a record of no known type (a Devise `resource`) still answers `class`.
    let mut class = Expr::new(span, ExprNode::Send { recv: Some(record), method: Symbol::from("class"), args: vec![], block: None, parenthesized: false });
    class.ty = Some(crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("Class")), args: vec![].into() });
    let mut name = Expr::new(span, ExprNode::Send { recv: Some(class), method: Symbol::from("name"), args: vec![], block: None, parenthesized: false });
    name.ty = Some(crate::ty::Ty::Str);
    let mut tables = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ErrorTables")] });
    tables.ty = Some(crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ErrorTables")), args: vec![].into() });
    let mut call = Expr::new(span, ExprNode::Send { recv: Some(tables), method: Symbol::from(table), args: vec![name], block: None, parenthesized: true });
    call.ty = Some(crate::ty::Ty::Hash { key: std::sync::Arc::new(crate::ty::Ty::Str), value: std::sync::Arc::new(crate::ty::Ty::Str) });
    call
}

/// `errors.import(error, attribute: :field)` → `ActiveSupport.errors_import(…)` and
/// `errors.merge!(other)` → `ActiveSupport.errors_merge(…)`, with the importing model's
/// translated attribute names: Rails names an imported error's attribute as the importer does.
fn rewrite_import(expr: &mut Expr, labels: &Labels, diags: &mut Vec<Diagnostic>) -> bool {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return false };
    let method = method.as_str();
    if !matches!(method, "import" | "merge!") || !is_errors_ty(reader) {
        return false;
    }
    let ExprNode::Send { recv: owner, method: em, args: ea, .. } = &*reader.node else { return false };
    if em.as_str() != "errors" || !ea.is_empty() {
        return false;
    }
    let i18n = labels.model(reader);
    let pieces = format_pieces(i18n.errors_format());
    let is_model = |id: &crate::ident::ClassId| labels.data.models.contains_key(id);
    let reason = if !import_grounds(method, args, &is_model) {
        Some(match method {
            "import" => "an error and an `attribute:` Symbol are grounded, no other option",
            _ => "only another record or its `errors` is grounded",
        })
    } else if !owner_reads_once(owner.as_ref()) {
        Some("the record is not a plain read, and the import reads it twice")
    } else if pieces.is_none() {
        Some("`errors.format` does not name the attribute and then the message, once each")
    } else {
        None
    };
    if let Some(reason) = reason {
        let mut d = crate::lower::residue_diagnostic(
            "errors_add",
            if method == "import" { "errors.import" } else { "errors.merge!" },
            expr.span,
            reason,
            format!("`errors.{method}` is not grounded: {reason}"),
        );
        d.severity = crate::diagnostic::Severity::Error;
        diags.push(d);
        return true;
    }
    let span = expr.span;
    let (before, between, after) = pieces.expect("checked above");
    let names: Vec<(Expr, Expr)> =
        i18n.translated_attribute_names().into_iter().map(|(attr, name)| (str_lit(span, &attr), str_lit(span, &name))).collect();
    let mut names = Expr::new(span, ExprNode::Hash { entries: names, kwargs: false });
    names.ty = Some(crate::ty::Ty::Hash { key: std::sync::Arc::new(crate::ty::Ty::Str), value: std::sync::Arc::new(crate::ty::Ty::Str) });
    let mut errors = reader.clone();
    errors.ty = Some(crate::ty::Ty::Array { elem: std::sync::Arc::new(crate::ty::Ty::Str) });
    let mut call_args = vec![errors, error_details(owner.clone(), span)];
    let helper = if method == "import" {
        let attribute = import_attribute(args).expect("checked above").map(|a| match &*a.node {
            ExprNode::Lit { value: Literal::Sym { value } } => str_lit(span, value.as_str()),
            _ => {
                let mut s = Expr::new(span, ExprNode::Send { recv: Some(a.clone()), method: Symbol::from("to_s"), args: vec![], block: None, parenthesized: false });
                s.ty = Some(crate::ty::Ty::Str);
                s
            }
        });
        call_args.push(args[0].clone());
        call_args.push(attribute.unwrap_or_else(|| {
            let mut nil = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
            nil.ty = Some(crate::ty::Ty::Nil);
            nil
        }));
        "errors_import"
    } else {
        call_args.push(error_details(merged_owner(args, &is_model).expect("checked above"), span));
        "errors_merge"
    };
    call_args.extend([names, str_lit(span, &before), str_lit(span, &between), str_lit(span, &after)]);
    let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
    support.ty = Some(crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveSupport")), args: vec![].into() });
    let mut call = Expr::new(
        span,
        ExprNode::Send { recv: Some(support), method: Symbol::from(helper), args: call_args, block: None, parenthesized: true },
    );
    call.ty = Some(crate::lower::errors_index::error_array_ty());
    *expr = call;
    true
}

/// `<errors> << msg` followed by `<owner>.error_details << ActiveRecord::ErrorDetail.new("field", type, message, msg, symbolic, optionless)`,
/// so the accumulator keeps what Rails keeps for each message.
pub(crate) fn attributed_push(
    push_message: Expr,
    owner: Option<Expr>,
    field: &str,
    kind: Expr,
    symbolic: bool,
    optionless: bool,
    message: Expr,
    span: crate::span::Span,
) -> Expr {
    let ExprNode::Send { args, .. } = &*push_message.node else { unreachable!("an `errors <<` push") };
    let full_message = typed_str(args[0].clone());
    let error_ty = crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveRecord::ErrorDetail")), args: vec![].into() };
    let mut class = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveRecord"), Symbol::from("ErrorDetail")] });
    class.ty = Some(error_ty.clone());
    let mut record = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(class),
            method: Symbol::from("new"),
            args: vec![str_lit(span, field), typed_str(kind), typed_str(message), full_message, bool_lit(span, symbolic), bool_lit(span, optionless)],
            block: None,
            parenthesized: true,
        },
    );
    record.ty = Some(error_ty);
    let mut push_detail = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(error_details(owner, span)),
            method: Symbol::from("<<"),
            args: vec![record],
            block: None,
            parenthesized: false,
        },
    );
    push_detail.ty = Some(error_details_ty());
    let ty = push_message.ty.clone();
    let mut both = Expr::new(span, ExprNode::Seq { exprs: vec![push_message, push_detail] });
    both.ty = ty;
    both
}

pub(crate) fn error_details_ty() -> crate::ty::Ty {
    crate::ty::Ty::Array {
        elem: std::sync::Arc::new(crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveRecord::ErrorDetail")), args: vec![].into() }),
    }
}

/// `<owner>.error_details`, typed.
pub(crate) fn error_details(owner: Option<Expr>, span: crate::span::Span) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: owner, method: Symbol::from("error_details"), args: Vec::new(), block: None, parenthesized: false },
    );
    e.ty = Some(error_details_ty());
    e
}

fn bool_lit(span: crate::span::Span, value: bool) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value } });
    e.ty = Some(crate::ty::Ty::Bool);
    e
}

fn str_lit(span: crate::span::Span, value: &str) -> Expr {
    typed_str(Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } }))
}

fn typed_str(mut e: Expr) -> Expr {
    if e.ty.is_none() {
        e.ty = Some(crate::ty::Ty::Str);
    }
    e
}

/// A message the push may evaluate twice, once for the full text and once for the record.
fn reads_without_effect_deep(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef | ExprNode::Const { .. } => true,
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            InterpPart::Text { .. } => true,
            InterpPart::Expr { expr } => reads_without_effect_deep(expr),
        }),
        // Not a zero-arg call either: nothing proves it leaves state alone, and it would run twice
        _ => false,
    }
}

/// `<errors reader>.full_message(:field, message)`: the one shape whose
/// text `rewrite_errors_add` bakes. Shared with analyze, which types it
/// `String` only when this holds.
pub(crate) fn full_message_bakes(recv: Option<&Expr>, args: &[Expr], block: Option<&Expr>) -> bool {
    block.is_none()
        && args.len() == 2
        && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
        && !matches!(&*args[1].node, ExprNode::Hash { .. })
        && recv.is_some_and(|r| matches!(&*r.node, ExprNode::Send { recv: owner, method, args, .. }
            if method.as_str() == "errors" && args.is_empty() && owner.as_ref().is_none_or(|o| reads_without_effect(o) || is_record(o))))
}

/// A record built for its `errors`, which `&&` evaluates before the text and which is never falsy.
fn is_record(e: &Expr) -> bool {
    matches!(&e.ty, Some(crate::ty::Ty::Class { .. }))
}

/// Folding drops the receiver, so only one whose evaluation does nothing.
fn reads_without_effect(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef)
}

/// The record an `errors` reader is on, when reading it again is the same record: self, or a plain read.
pub(crate) fn owner_reads_once(owner: Option<&Expr>) -> bool {
    // A bare zero-arg call counts: a view's or partial's local reaches here in that shape
    owner.is_none_or(|o| {
        reads_without_effect(o)
            || matches!(&*o.node, ExprNode::Send { recv: None, args, block: None, .. } if args.is_empty())
    })
}

/// A looked-up message as a string: a literal when every part is known,
/// else an interpolation reading `field` off `owner` (self when `None`)
/// where `%{value}` stood.
pub(crate) fn parts_expr(
    span: crate::span::Span,
    parts: Vec<crate::i18n::Part>,
    owner: Option<&Expr>,
    field: &str,
) -> Expr {
    if let [crate::i18n::Part::Text(text)] = parts.as_slice() {
        return Expr::new(span, ExprNode::Lit { value: Literal::Str { value: text.clone() } });
    }
    let parts = parts
        .into_iter()
        .map(|p| match p {
            crate::i18n::Part::Text(value) => InterpPart::Text { value },
            crate::i18n::Part::Value => InterpPart::Expr {
                expr: Expr::new(
                    span,
                    ExprNode::Send {
                        recv: owner.cloned(),
                        method: Symbol::from(field),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
            },
        })
        .collect();
    Expr::new(span, ExprNode::StringInterp { parts })
}

/// The full message Rails stores for `field` given a String message:
/// `errors.format` over the attribute's human name, the bare message for
/// `:base`.
fn baked_message(
    i18n: &crate::i18n::ModelI18n,
    span: crate::span::Span,
    field: &str,
    msg: Option<Expr>,
) -> Expr {
    let lit = |value: String| Expr::new(span, ExprNode::Lit { value: Literal::Str { value } });
    let Some(msg) = msg else {
        return parts_expr(span, i18n.full_error(field, "invalid", &Default::default()), None, field);
    };
    if field == "base" {
        return msg;
    }
    if let ExprNode::Lit { value: Literal::Str { value } } = &*msg.node {
        return lit(i18n.full_message(field, value));
    }
    // A dynamic message splices into the format where `%{message}` stands.
    let format = i18n.errors_format().replace("%{attribute}", &i18n.human_attribute_name(field));
    let (before, after) = format.split_once("%{message}").unwrap_or((format.as_str(), ""));
    let mut parts = vec![InterpPart::Text { value: before.to_string() }, InterpPart::Expr { expr: msg }];
    if !after.is_empty() {
        parts.push(InterpPart::Text { value: after.to_string() });
    }
    Expr::new(span, ExprNode::StringInterp { parts })
}
