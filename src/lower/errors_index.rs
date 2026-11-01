//! `errors[:field]` grounding: `<recv>.errors[:field]` →
//! `<recv>.error_messages_for("field")`.
//!
//! The shared runtime's `errors` is a plain `Array[String]` of FULL
//! messages, and `error_details` holds the `ActiveRecord::ErrorDetail`
//! each was added with — [`super::errors_add`] and the `validates`
//! lowering push both. Rails' `errors[:field]` is the messages on one
//! attribute WITHOUT its humanized name (`[ "is not public" ]`, not
//! `[ "Url is not public" ]`), which `Base#error_messages_for` answers
//! from the details, under any `errors.format` and for `:base` alike.
//!
//! Left alone the site is not merely unsupported, it is silently wrong
//! on CRuby too: `Array#[]` with a Symbol raises `TypeError: no implicit
//! conversion of Symbol into Integer`, which is what campfire's
//! `Opengraph::LocationTest` reported for three tests.
//!
//! Purely shape-directed, like its two siblings: the receiver of `[]`
//! must itself be a zero-arg `errors` send, which is what makes the
//! site our accumulator rather than some other object's `[]`.
//!
//! `errors.to_hash` is the same projection for every attribute at once:
//! `ActiveSupport.errors_to_hash(<recv>.error_details)` groups the
//! details' messages by attribute, in the order Rails does. A
//! `messages` the per-field rewrites leave is that Hash too, through
//! `errors_messages`, and `attribute_names` is
//! `errors_attribute_names`. `include?` / `key?` / `has_key?` ask
//! `error_messages_for` like `errors[:field]` does.
//!
//! Iterating `errors` (`each`, `map`, `find`, `first`, …) iterates
//! `error_details`, which Rails' `ActiveModel::Error` reads are asked of:
//! `attribute` answers a Symbol, `message` and `full_message` the texts.
//! `details` groups the same records by attribute, and an entry's
//! `[:error]` reads as `type` does; its options are not kept, so no
//! other key is grounded. `added?` is `of_kind?` asked only of errors
//! Rails keeps no options for.
//!
//! Views take the same rewrite: the scaffold's form partial reads
//! `<record>.errors[:field].none?` / `.any?` inside a `class:` array,
//! which renders through `class_names` at request time.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// Rewrite `errors[:field]` reads across every hook, test and view body.
/// Returns the residue ledger:
/// recognizable `errors[…]` sites left dynamic, with the reason.
pub fn apply_errors_index_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_owned_hook_body(app, &mut |_, body| rewrite_body(body, &mut diags));
    super::for_each_test_body(app, &mut |body| rewrite_body(body, &mut diags));
    for view in &mut app.views {
        rewrite_body(&mut view.body, &mut diags);
    }
    diags
}

fn rewrite_body(body: &mut Expr, diags: &mut Vec<Diagnostic>) {
    rewrite(body, diags);
    rewrite_whole_messages(body);
    read_errors(body, diags);
    iterate_details(body);
}

/// What `errors` answers through Enumerable, over its `ActiveModel::Error`s.
pub(crate) const ERROR_ITERATORS: &[&str] =
    &["each", "map", "collect", "select", "filter", "reject", "find", "detect", "any?", "all?", "none?", "count", "first"];

fn is_error(e: &Expr) -> bool {
    matches!(e.ty.as_ref().map(Ty::peel_nilable), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Error")
}

/// What analyze types an entry of `errors.details` as: Rails' Hash of the type and the error's options.
pub(crate) const DETAILS_ENTRY: &str = "ActiveModel::ErrorDetails";

fn is_details_entry(e: &Expr) -> bool {
    matches!(e.ty.as_ref().map(Ty::peel_nilable), Some(Ty::Class { id, .. }) if id.0.as_str() == DETAILS_ENTRY)
}

/// `[:error]`, the one key of a `details` entry that is grounded.
pub(crate) fn reads_detail_type(args: &[Expr]) -> bool {
    matches!(args, [key] if sym_lit(key).is_some_and(|k| k.as_str() == "error"))
}

/// `ActiveModel::Error#attribute` and `#type` over the `ErrorDetail` an iteration yields.
fn read_errors(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| read_errors(c, diags));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else { return };
    let entry_type = method.as_str() == "[]" && is_details_entry(r) && reads_detail_type(args);
    if !entry_type && (!args.is_empty() || !is_error(r)) {
        return;
    }
    let span = expr.span;
    match if entry_type { "type" } else { method.as_str() } {
        // Not `ErrorDetail#attribute` as it is: it is the attribute's String, where Rails' `Error#attribute` is a Symbol.
        "attribute" => {
            expr.ty = Some(Ty::Str);
            let read = std::mem::replace(expr, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
            *expr = send(span, read, "to_sym", vec![], Ty::Sym);
        }
        // Rails' type is a Symbol, or the message when `errors.add` was given a String.
        "type" => {
            if !reads_again(r) {
                let mut d = crate::lower::residue_diagnostic(
                    "errors_index",
                    "error.type",
                    span,
                    "the error is not a plain read",
                    "`error.type` and a `details` entry's `[:error]` read the error three times, so only on a block parameter, a local or `errors.first`".to_string(),
                );
                d.severity = crate::diagnostic::Severity::Error;
                diags.push(d);
                return;
            }
            let error = r.clone();
            let kind = || send(span, error.clone(), "kind", vec![], Ty::Str);
            let mut read = Expr::new(
                span,
                ExprNode::If {
                    cond: send(span, error.clone(), "symbolic", vec![], Ty::Bool),
                    then_branch: send(span, kind(), "to_sym", vec![], Ty::Sym),
                    else_branch: kind(),
                },
            );
            read.ty = Some(Ty::Union { variants: vec![Ty::Sym, Ty::Str].into() });
            *expr = read;
        }
        _ => {}
    }
}

fn reads_again(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Var { .. } | ExprNode::Ivar { .. } => true,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } => method.as_str() == "first" && args.is_empty() && is_errors_read(r),
        _ => false,
    }
}

/// `<owner>.errors.<iterator>` → `<owner>.error_details.<iterator>`, and
/// every `ActiveModel::Error` it yields or answers is an `ErrorDetail`.
fn iterate_details(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| iterate_details(c));
    let error = crate::ident::ClassId(Symbol::from("ActiveModel::Error"));
    let entry = crate::ident::ClassId(Symbol::from(DETAILS_ENTRY));
    let detail = crate::ident::ClassId(Symbol::from("ActiveRecord::ErrorDetail"));
    if let Some(ty) = &expr.ty {
        expr.ty = Some(ty.rebind_class(&error, &detail).rebind_class(&entry, &detail));
    }
    let ExprNode::Send { recv: Some(reader), method, block, .. } = &mut *expr.node else { return };
    if !ERROR_ITERATORS.contains(&method.as_str()) || !is_model_errors_read(reader) {
        return;
    }
    // Their own answers when blockless are the accumulator's: the count, or whether there is any message.
    if block.is_none() && matches!(method.as_str(), "each" | "any?" | "none?" | "count") {
        return;
    }
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    *reader = super::errors_add::error_details(owner.clone(), reader.span);
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "errors_index",
        "errors[]",
        expr.span,
        reason,
        format!(
            "`errors[:field]` left as dynamic dispatch ({reason}) — the error \
             accumulator is an Array[String] whose `[]` takes an Integer; \
             ground by hand or extend the errors_index lowering"
        ),
    )
}

/// Is this expression a zero-arg, block-free `errors` send — i.e. our
/// accumulator? The receiver of `errors` itself is unconstrained, so
/// both `errors[:x]` (a model reading its own) and `record.errors[:x]`
/// (a test reading another object's) match.
fn is_errors_read(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { method, args, block: None, .. }
            if method.as_str() == "errors" && args.is_empty()
    )
}

// Not shape alone, as `[]` is: an app's own `errors` Array answers `include?` and `attribute_names` itself.
fn is_model_errors_read(e: &Expr) -> bool {
    is_errors_read(e) && matches!(&e.ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
}

fn is_messages_read(e: &Expr) -> bool {
    matches!(&*e.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "messages" && args.is_empty() && is_errors_read(r))
}

fn sym_lit(e: &Expr) -> Option<&Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}

fn send(span: crate::span::Span, recv: Expr, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: false },
    );
    e.ty = Some(ty);
    e
}

// Not a Hash: the accumulator is the Array of full messages, so each `messages` question is asked of it directly.
fn rewrite_messages(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let span = expr.span;
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else { return };
    if !is_messages_read(r) {
        return;
    }
    let ExprNode::Send { recv: Some(errors), .. } = &*r.node else { return };
    let accumulator = errors.clone();
    // A field that is not a Symbol literal reads the whole Hash instead, which `rewrite_whole_messages` builds.
    let replacement = match (method.as_str(), args.as_slice()) {
        ("[]", [field]) if sym_lit(field).is_some() => {
            *r = accumulator;
            None
        }
        ("key?" | "has_key?" | "include?" | "member?", [field]) if sym_lit(field).is_some() => {
            let field = field.clone();
            let mut index = send(span, accumulator, "[]", vec![field], Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
            rewrite_index(&mut index, diags);
            Some(send(span, index, "any?", vec![], Ty::Bool))
        }
        ("blank?" | "empty?", []) => Some(send(span, accumulator, "empty?", vec![], Ty::Bool)),
        ("present?" | "any?", []) => Some(send(span, accumulator, "any?", vec![], Ty::Bool)),
        _ => None,
    };
    if let Some(new) = replacement {
        *expr = new;
    }
}

fn rewrite(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, diags));
    rewrite_messages(expr, diags);
    rewrite_index(expr, diags);
    rewrite_include(expr, diags);
    rewrite_of_kind(expr, diags);
    rewrite_delete(expr, diags);
    rewrite_where(expr, diags);
    rewrite_to_hash(expr);
    rewrite_render_json(expr);
}

// Not left to the accumulator: `Array#include?(:title)` answers false for an Array of full messages.
fn rewrite_include(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if !matches!(method.as_str(), "include?" | "key?" | "has_key?") || args.len() != 1 || !is_model_errors_read(reader) {
        return;
    }
    let field = match &*args[0].node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    };
    let Some(field) = field else {
        let mut d = crate::lower::residue_diagnostic(
            "errors_index",
            "errors.include?",
            expr.span,
            "dynamic field",
            format!(
                "`errors.{method}` with a field that is not a literal is not grounded; the error \
                 accumulator's own `{method}` would ask its full messages"
            ),
        );
        d.severity = crate::diagnostic::Severity::Error;
        diags.push(d);
        return;
    };
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    *expr = has_errors_on(owner.clone(), &field, expr.span);
}

/// The field `errors.delete` removes, when it is the only argument and a literal.
pub(crate) fn delete_field(args: &[Expr]) -> Option<String> {
    match args {
        [field] => match &*field.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `errors.delete(:field)` → `ActiveSupport.errors_delete(<owner>.errors, <owner>.error_details, "field")`.
fn rewrite_delete(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if method.as_str() != "delete" || !is_model_errors_read(reader) {
        return;
    }
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    let (Some(field), true) = (delete_field(args), super::errors_add::owner_reads_once(owner.as_ref())) else {
        let mut d = crate::lower::residue_diagnostic(
            "errors_index",
            "errors.delete",
            expr.span,
            "not a literal field on a plain read",
            "`errors.delete` is grounded only with a literal field, no type, on a record read without effect".to_string(),
        );
        d.severity = crate::diagnostic::Severity::Error;
        diags.push(d);
        return;
    };
    let span = expr.span;
    let mut call = details_call(span, reader, "errors_delete", Ty::Union { variants: vec![Ty::Array { elem: std::sync::Arc::new(Ty::Str) }, Ty::Nil].into() });
    let ExprNode::Send { args, .. } = &mut *call.node else { unreachable!() };
    let mut errors = reader.clone();
    errors.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
    args.insert(0, errors);
    args.push(str_expr(span, &field));
    *expr = call;
}

/// The field, whether the type is a Symbol, and the type, when each is a literal.
fn of_kind_args(args: &[Expr]) -> Option<(String, bool, String)> {
    let field = match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => value.as_str().to_string(),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => value.clone(),
        _ => return None,
    };
    let (symbolic, kind) = match args.get(1).map(|a| &*a.node) {
        None => (true, "invalid".to_string()),
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => (true, value.as_str().to_string()),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => (false, value.clone()),
        _ => return None,
    };
    (args.len() <= 2).then_some((field, symbolic, kind))
}

/// Shared with analyze, which types `of_kind?` only when this holds.
pub(crate) fn of_kind_grounds(args: &[Expr]) -> bool {
    of_kind_args(args).is_some()
}

/// `errors.of_kind?(:field, :type)`: an error on the field with that Symbol type; a String type is one of the field's messages, as Rails asks it.
fn rewrite_of_kind(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if !matches!(method.as_str(), "of_kind?" | "added?") || !is_model_errors_read(reader) {
        return;
    }
    // Rails' `added?` is `strict_match?`: the error's options, `count:` or `value:`, must be the ones asked, and none are.
    let strict = method.as_str() == "added?";
    let Some((field, symbolic, kind)) = of_kind_args(args) else {
        let mut d = crate::lower::residue_diagnostic(
            "errors_index",
            if strict { "errors.added?" } else { "errors.of_kind?" },
            expr.span,
            "not literal",
            format!("`errors.{method}` with a field or type that is not a literal, or with options, is not grounded"),
        );
        d.severity = crate::diagnostic::Severity::Error;
        diags.push(d);
        return;
    };
    let span = expr.span;
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    if !symbolic {
        let messages = messages_on(owner.clone(), &field, span);
        *expr = send(span, messages, "include?", vec![str_expr(span, &kind)], Ty::Bool);
        let ExprNode::Send { parenthesized, .. } = &mut *expr.node else { unreachable!() };
        *parenthesized = true;
        return;
    }
    *expr = details_filter(span, owner.clone(), "any?", &field, Some((symbolic, &kind)), strict, Ty::Bool);
}

/// `<owner>.error_details.<method> { |error| error.attribute == "field" && <type matches> }`, typed.
fn details_filter(span: crate::span::Span, owner: Option<Expr>, method: &str, field: &str, kind: Option<(bool, &str)>, optionless: bool, ty: Ty) -> Expr {
    let detail_ty = Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveRecord::ErrorDetail")), args: vec![].into() };
    let error = || {
        let mut e = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("error") });
        e.ty = Some(detail_ty.clone());
        e
    };
    let and = |left: Expr, right: Expr| {
        let mut e = Expr::new(span, ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, surface: crate::expr::BoolOpSurface::Symbol, left, right });
        e.ty = Some(Ty::Bool);
        e
    };
    let equals = |reader: &str, value: &str| {
        send(span, send(span, error(), reader, vec![], Ty::Str), "==", vec![str_expr(span, value)], Ty::Bool)
    };
    let mut body = equals("attribute", field);
    if let Some((symbolic, kind)) = kind {
        let mut typed = send(span, error(), "symbolic", vec![], Ty::Bool);
        if !symbolic {
            typed = send(span, typed, "!", vec![], Ty::Bool);
        }
        body = and(and(body, typed), equals("kind", kind));
    }
    if optionless {
        body = and(body, send(span, error(), "optionless", vec![], Ty::Bool));
    }
    let block = Expr::new(
        span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            rest_param: None,
            params: vec![Symbol::from("error")],
            block_param: None,
            body,
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    let mut call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(super::errors_add::error_details(owner, span)),
            method: Symbol::from(method),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    call.ty = Some(ty);
    call
}

/// The field and the type `errors.where` selects on, when each is a literal.
pub(crate) fn where_args(args: &[Expr]) -> Option<(String, Option<(bool, String)>)> {
    let field = match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => value.as_str().to_string(),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => value.clone(),
        _ => return None,
    };
    let kind = match args.get(1).map(|a| &*a.node) {
        None => None,
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some((true, value.as_str().to_string())),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => Some((false, value.clone())),
        _ => return None,
    };
    (args.len() <= 2).then_some((field, kind))
}

/// `errors.where(:field)` and `where(:field, :type)`: the field's errors, of that type when one is given, as Rails' `Error#match?` picks them.
fn rewrite_where(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if method.as_str() != "where" || !is_model_errors_read(reader) {
        return;
    }
    let Some((field, kind)) = where_args(args) else {
        let mut d = crate::lower::residue_diagnostic(
            "errors_index",
            "errors.where",
            expr.span,
            "not literal",
            "`errors.where` with a field or type that is not a literal, or with options, is not grounded".to_string(),
        );
        d.severity = crate::diagnostic::Severity::Error;
        diags.push(d);
        return;
    };
    let span = expr.span;
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    let ty = expr.ty.clone().unwrap_or_else(error_array_ty);
    *expr = details_filter(span, owner.clone(), "select", &field, kind.as_ref().map(|(s, k)| (*s, k.as_str())), false, ty);
}

pub(crate) fn error_array_ty() -> Ty {
    Ty::Array { elem: std::sync::Arc::new(Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveModel::Error")), args: vec![].into() }) }
}

fn str_expr(span: crate::span::Span, value: &str) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } });
    e.ty = Some(Ty::Str);
    e
}

/// What the per-field rewrites leave of `errors.messages` is the Hash itself.
fn rewrite_whole_messages(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_whole_messages(c));
    if !is_messages_read(expr) {
        return;
    }
    let ExprNode::Send { recv: Some(reader), .. } = &*expr.node else { unreachable!("a messages read") };
    *expr = details_call(expr.span, reader, "errors_messages", messages_ty());
}

/// `<owner>.errors.to_hash` and `.as_json` → `ActiveSupport.errors_to_hash(<owner>.error_details)`,
/// `.attribute_names` → `ActiveSupport.errors_attribute_names(…)`, typed.
fn rewrite_to_hash(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(reader), method, args, block: None, .. } = &*expr.node else { return };
    if !args.is_empty() || !is_errors_read(reader) {
        return;
    }
    let (helper, ty) = match method.as_str() {
        "to_hash" | "as_json" => ("errors_to_hash", messages_ty()),
        "attribute_names" if is_model_errors_read(reader) => ("errors_attribute_names", Ty::Array { elem: std::sync::Arc::new(Ty::Sym) }),
        "details" if is_model_errors_read(reader) => ("errors_details", details_ty()),
        _ => return,
    };
    *expr = details_call(expr.span, reader, helper, ty);
}

// Not the Array of full messages, nor `errors_to_hash`: Rails' `render json:` writes `errors.as_json` keyed by attribute, and `Base#errors_json` reaches every target with String keys, which JSON writes the same.
fn rewrite_render_json(expr: &mut Expr) {
    let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
    if method.as_str() != "render" {
        return;
    }
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (key, value) in entries.iter_mut() {
            if matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "json") {
                json_value(value);
            }
        }
    }
}

fn json_value(value: &mut Expr) {
    let reader = match &*value.node {
        // Not a folded `errors.full_messages`: that pass leaves the
        // `errors` read typed as the messages it answers, an Array, and
        // Rails writes that Array.
        _ if is_errors_read(value) && !matches!(value.ty, Some(Ty::Array { .. })) => Some(&*value),
        ExprNode::Send { recv: Some(r), .. } if is_messages_read(value) => Some(r),
        _ => None,
    };
    if let Some(reader) = reader {
        let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
        let mut json = Expr::new(
            value.span,
            ExprNode::Send { recv: owner.clone(), method: Symbol::from("errors_json"), args: Vec::new(), block: None, parenthesized: false },
        );
        json.ty = Some(Ty::Hash {
            key: std::sync::Arc::new(Ty::Str),
            value: std::sync::Arc::new(Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
        });
        *value = json;
        return;
    }
    match &mut *value.node {
        ExprNode::Hash { entries, .. } => entries.iter_mut().for_each(|(_, v)| json_value(v)),
        ExprNode::Array { elements, .. } => elements.iter_mut().for_each(json_value),
        _ => {}
    }
}

/// `Hash[Symbol, Array[<a details entry>]]`.
pub(crate) fn details_ty() -> Ty {
    let entry = Ty::Class { id: crate::ident::ClassId(Symbol::from(DETAILS_ENTRY)), args: vec![].into() };
    Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Array { elem: std::sync::Arc::new(entry) }) }
}

fn messages_ty() -> Ty {
    Ty::Hash {
        key: std::sync::Arc::new(Ty::Sym),
        value: std::sync::Arc::new(Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
    }
}

/// `ActiveSupport.<helper>(<owner>.error_details)`, typed.
fn details_call(span: crate::span::Span, reader: &Expr, helper: &str, ty: Ty) -> Expr {
    let ExprNode::Send { recv: owner, .. } = &*reader.node else { unreachable!("an errors reader") };
    let mut details = Expr::new(
        span,
        ExprNode::Send { recv: owner.clone(), method: Symbol::from("error_details"), args: Vec::new(), block: None, parenthesized: false },
    );
    details.ty = Some(super::errors_add::error_details_ty());
    let mut support = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
    support.ty = Some(Ty::Class { id: crate::ident::ClassId(Symbol::from("ActiveSupport")), args: vec![].into() });
    let mut call = Expr::new(
        span,
        ExprNode::Send { recv: Some(support), method: Symbol::from(helper), args: vec![details], block: None, parenthesized: true },
    );
    call.ty = Some(ty);
    call
}

fn rewrite_index(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    let matches_shape = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "[]" && args.len() == 1 && is_errors_read(r)
    );
    if !matches_shape {
        return;
    }
    let field = match &*expr.node {
        ExprNode::Send { args, .. } => match &*args[0].node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            _ => None,
        },
        _ => unreachable!(),
    };
    let Some(field) = field else {
        diags.push(residue(expr, "dynamic field"));
        return;
    };
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, .. } = node else { unreachable!() };
    let ExprNode::Send { recv: owner, .. } = *recv.expect("checked Some above").node else { unreachable!() };
    *expr = messages_on(owner, field.as_str(), span);
}

/// `<owner>.error_messages_for("<field>")`, typed.
pub(crate) fn messages_on(owner: Option<Expr>, field: &str, span: crate::span::Span) -> Expr {
    let mut lit = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: field.to_string() } });
    lit.ty = Some(Ty::Str);
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: owner, method: Symbol::from("error_messages_for"), args: vec![lit], block: None, parenthesized: true },
    );
    // Stamp the result: callers run *after* analyze, and the post-lowering
    // `diagnose` walk reads stamped types without re-dispatching.
    e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
    e
}

/// `<owner>.error_messages_for("<field>").any?`, typed.
pub(crate) fn has_errors_on(owner: Option<Expr>, field: &str, span: crate::span::Span) -> Expr {
    send(span, messages_on(owner, field, span), "any?", vec![], Ty::Bool)
}
