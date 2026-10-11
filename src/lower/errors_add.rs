//! `errors.add` grounding: `errors.add(:field, "msg")` →
//! `errors << "Field msg"`.
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
//! arguments. Only that shape is typed (`full_message_bakes`); any other
//! spelling keeps analyze's Errors gap.
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
    diags
}

/// Each model's stamped i18n, and the app's for an `errors` whose model
/// is not known, held apart from the `App` the walk borrows mutably.
pub(crate) struct LabelData {
    models: std::collections::HashMap<crate::ident::ClassId, crate::i18n::ModelI18n>,
    unknown: crate::i18n::ModelI18n,
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
    /// The text a full message for `field` starts with, before its message;
    /// `None` when `errors.format` does not end in the message.
    pub(crate) fn message_prefix(&self, errors_reader: &Expr, field: &str) -> Option<String> {
        let i18n = self.model(errors_reader);
        let format = i18n.errors_format().replace("%{attribute}", &i18n.human_attribute_name(field));
        format.strip_suffix("%{message}").filter(|p| !p.contains("%{message}")).map(str::to_string)
    }

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
            let message = baked_message(i18n, span, field.as_str(), args.next().map(fold_str_concat));
            *expr = message;
            expr.ty = Some(crate::ty::Ty::Str);
            return;
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
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { args, .. }
            if !args.is_empty()
                && args.len() <= 2
                && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { .. } })
    );
    if !matches {
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
        diags.push(residue(expr, "unrecognized arg shape"));
        *expr.node = ExprNode::Send {
            recv,
            method: Symbol::from("add"),
            args: vec![field_expr, msg.unwrap()],
            block: None,
            parenthesized: true,
        };
        return;
    }
    let reader = recv.as_ref().expect("checked errors reader");
    let i18n = labels.model(reader);
    // Rails looks a Symbol message (or none, `:invalid`) up as an error type.
    let kind = match msg.as_ref().map(|m| &*m.node) {
        None => Some("invalid".to_string()),
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.as_str().to_string()),
        _ => None,
    };
    let message = match kind {
        Some(kind) => {
            let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!() };
            parts_expr(span, i18n.full_error(field.as_str(), &kind, &Default::default()), owner.as_ref(), field.as_str())
        }
        None => baked_message(i18n, span, field.as_str(), msg),
    };
    // `<<` returns the accumulator it appends to (Array#<< / the errors
    // collection), so the synthesized send's result type is the
    // receiver's. Stamp it: this pass runs *after* analyze, and the
    // post-lowering `diagnose` walk reads stamped types without re-
    // dispatching — leaving `ty = None` here makes it false-positive a
    // `send_dispatch_failed` on a receiver it can see the type of.
    let recv_ty = recv.as_ref().and_then(|r| r.ty.clone());
    *expr.node = ExprNode::Send {
        recv,
        method: Symbol::from("<<"),
        args: vec![message],
        block: None,
        parenthesized: false,
    };
    expr.ty = recv_ty;
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
            if method.as_str() == "errors" && args.is_empty() && owner.as_ref().is_none_or(reads_without_effect)))
}

/// Folding drops the receiver, so only one whose evaluation does nothing.
fn reads_without_effect(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef)
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
