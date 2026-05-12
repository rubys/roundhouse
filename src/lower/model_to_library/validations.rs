//! Validations: lower `validates :attr, presence: true, length: { ... }` into
//! a single `def validate` body. Each rule expands to inline IR (`if cond
//! then errors << "msg" end`) rather than a helper-call into the runtime
//! Validations module — the Phase 2.5(a) lowerer per docs/rust-migration-plan.md.
//!
//! Inline expansion wins three ways: (1) error messages are string-literal
//! constants, no runtime interpolation, (2) typed targets (Rust, Crystal,
//! strict TS) avoid the `untyped value` channel that the Validations module
//! forces, (3) every target gets the same expansion — no per-target adapter
//! for the Validations module's dispatch shape.
//!
//! One top-level `def validate` per model; rules across multiple attrs each
//! append their own stmt block to the body.

use crate::dialect::{AccessorKind, Association, MethodDef, MethodReceiver, Model, ValidationRule};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::{fn_sig, lit_float, lit_int, lit_sym, seq};

pub(super) fn push_validate_method(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for v in model.validations() {
        // Column type from the model's attributes row (when present).
        // Lets the per-rule generator skip dead `is_a?(Array)` branches
        // for fields the schema declares as `Str` — tsc narrows
        // `Array.isArray(stringField)` to `never` and rejects the
        // subsequent `.length` access.
        let attr_ty = model.attributes.fields.get(&v.attribute);
        for rule in &v.rules {
            stmts.extend(validation_rule_to_calls(&v.attribute, rule, attr_ty));
        }
    }

    // Rails 5+ default: every `belongs_to` requires the associated
    // record to exist before save. Emit `validates_belongs_to(:assoc,
    // @<fk>, <Target>)` per non-optional belongs_to. The runtime
    // helper short-circuits when the FK is unset (nil/0) and queries
    // `<Target>.exists?(fk_value)` otherwise.
    for assoc in model.associations() {
        if let Association::BelongsTo { name, target, foreign_key, optional: false } = assoc {
            stmts.push(inline_belongs_to_check(name, foreign_key, target));
        }
    }

    if stmts.is_empty() {
        return;
    }

    methods.push(MethodDef {
        name: Symbol::from("validate"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: seq(stmts),
        signature: Some(fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
    });
}

/// Produce the list of helper-call expressions for one `ValidationRule` on
/// `attr`. Each helper is `<helper>(:attr, @attr [, kwargs])` — the value
/// is passed positionally so the runtime helper sees a concretely-typed
/// `value` parameter (no block-yield, no `instance_variable_get`).
fn validation_rule_to_calls(attr: &Symbol, rule: &ValidationRule, attr_ty: Option<&Ty>) -> Vec<Expr> {
    match rule {
        ValidationRule::Presence => vec![inline_presence_check(attr, attr_ty)],
        ValidationRule::Absence => vec![inline_absence_check(attr)],
        ValidationRule::Length { min, max } => {
            inline_length_check(attr, min.map(|n| n as usize), max.map(|n| n as usize), attr_ty)
        }
        ValidationRule::Format { pattern } => vec![inline_format_check(attr, pattern)],
        ValidationRule::Numericality { only_integer, gt, lt } => {
            inline_numericality_check(attr, *only_integer, *gt, *lt)
        }
        ValidationRule::Inclusion { values } => vec![inline_inclusion_check(attr, values)],
        ValidationRule::Uniqueness { .. } | ValidationRule::Custom { .. } => {
            // Not yet exercised by real-blog; lands when a fixture forces the issue.
            Vec::new()
        }
    }
}

fn helper_call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(name),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// Inline `belongs_to` presence check (Rails 5+ default — every
/// non-optional `belongs_to` requires the associated record to
/// exist). Generates the IR equivalent of:
///   if @article_id.nil? || @article_id == 0 || !Article.exists?(@article_id)
///     errors << "article must exist"
///   end
/// Mirrors `runtime/ruby/active_record/validations.rb::validates_belongs_to`
/// but flattens the early-return + post-check sequence to a single
/// composite condition.
fn inline_belongs_to_check(
    assoc_name: &Symbol,
    foreign_key: &Symbol,
    target: &ClassId,
) -> Expr {
    let fk_ivar = ivar(foreign_key);
    // `@fk.nil?`
    let nil_check = send(fk_ivar.clone(), "nil?", vec![]);
    // `@fk == 0`
    let zero_check = send(
        fk_ivar.clone(),
        "==",
        vec![Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Int { value: 0 } },
        )],
    );
    // `!Target.exists?(@fk)`
    let target_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![target.0.clone()] },
    );
    let exists_call = send(target_const, "exists?", vec![fk_ivar]);
    let not_exists = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(exists_call),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let cond = bool_op(
        BoolOpKind::Or,
        bool_op(BoolOpKind::Or, nil_check, zero_check),
        not_exists,
    );
    let push_err = errors_push(format!("{} must exist", assoc_name.as_str()));
    if_with_nil_else(cond, push_err)
}

/// `@<attr>` — direct ivar read passed as the `value` positional arg
/// to every validates_* helper.
fn ivar(attr: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: attr.clone() })
}

/// Inline `validates :attr, presence: true` expansion.
/// Generates the IR equivalent of:
///   if @attr.nil? || (@attr.is_a?(String) && @attr.empty?) || (@attr.is_a?(Array) && @attr.empty?)
///     errors << "attr can't be blank"
///   end
///
/// Mirrors `runtime/ruby/active_record/validations.rb::validates_presence_of`
/// exactly; no behavior change vs. the helper-call form, but the
/// expansion lives in the IR so every target emits inline checks
/// instead of routing through the Validations module at runtime.
/// When `attr_ty` is `Some(Ty::Str)` the `is_a?(Array)` arm drops out
/// (typed targets like TS narrow `Array.isArray(stringField)` to
/// `never` and reject the subsequent property access). Same logic for
/// `Some(Ty::Array { .. })` — the String arm drops. `None` keeps the
/// generic three-way form so untyped/dynamic-shape attrs still work.
fn inline_presence_check(attr: &Symbol, attr_ty: Option<&Ty>) -> Expr {
    let attr_ivar = ivar(attr);
    // `@attr.nil?`
    let nil_check = send(attr_ivar.clone(), "nil?", vec![]);
    let cond = match attr_ty {
        Some(Ty::Str) => {
            // Skip is_a?(Array) — `body : String` can never be an array.
            bool_op(
                BoolOpKind::Or,
                nil_check,
                send(attr_ivar, "empty?", vec![]),
            )
        }
        Some(Ty::Array { .. }) => {
            // Skip is_a?(String) — symmetric.
            bool_op(
                BoolOpKind::Or,
                nil_check,
                send(attr_ivar, "empty?", vec![]),
            )
        }
        _ => {
            // Generic: `@attr.nil? || (@attr.is_a?(String) && @attr.empty?) ||
            //          (@attr.is_a?(Array) && @attr.empty?)`
            let string_blank = bool_op(
                BoolOpKind::And,
                is_a_check(&attr_ivar, "String"),
                send(attr_ivar.clone(), "empty?", vec![]),
            );
            let array_blank = bool_op(
                BoolOpKind::And,
                is_a_check(&attr_ivar, "Array"),
                send(attr_ivar, "empty?", vec![]),
            );
            bool_op(
                BoolOpKind::Or,
                bool_op(BoolOpKind::Or, nil_check, string_blank),
                array_blank,
            )
        }
    };
    // `errors << "attr can't be blank"`
    let push_err = errors_push(format!("{} can't be blank", attr.as_str()));
    // The wrapping `if cond then push_err end` (Nil else).
    if_with_nil_else(cond, push_err)
}

/// Inline `validates :attr, absence: true` — the negation of presence.
///   if !(@attr.nil? || (@attr.is_a?(String) && @attr.empty?) || (@attr.is_a?(Array) && @attr.empty?))
///     errors << "attr must be blank"
///   end
/// Reuses the presence condition tree and wraps with unary `!`.
fn inline_absence_check(attr: &Symbol) -> Expr {
    // Re-derive the blank-condition (matches inline_presence_check's tree).
    let attr_ivar = ivar(attr);
    let nil_check = send(attr_ivar.clone(), "nil?", vec![]);
    let string_blank = bool_op(
        BoolOpKind::And,
        is_a_check(&attr_ivar, "String"),
        send(attr_ivar.clone(), "empty?", vec![]),
    );
    let array_blank = bool_op(
        BoolOpKind::And,
        is_a_check(&attr_ivar, "Array"),
        send(attr_ivar, "empty?", vec![]),
    );
    let blank_cond = bool_op(
        BoolOpKind::Or,
        bool_op(BoolOpKind::Or, nil_check, string_blank),
        array_blank,
    );
    let not_blank = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(blank_cond),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err = errors_push(format!("{} must be blank", attr.as_str()));
    if_with_nil_else(not_blank, push_err)
}

/// Inline `validates :attr, inclusion: { in: [v1, v2, …] }`.
///   if ![v1, v2, …].include?(@attr)
///     errors << "attr is not included in the list"
///   end
/// The `within.nil?` guard from the runtime helper is unnecessary —
/// the list is a known literal at lower time.
fn inline_inclusion_check(attr: &Symbol, values: &[Literal]) -> Expr {
    let array_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Array {
            elements: values
                .iter()
                .map(|lit| Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() }))
                .collect(),
            style: ArrayStyle::Brackets,
        },
    );
    let attr_ivar = ivar(attr);
    let include_call = send(array_lit, "include?", vec![attr_ivar]);
    let not_included = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(include_call),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err = errors_push(format!("{} is not included in the list", attr.as_str()));
    if_with_nil_else(not_included, push_err)
}

/// Inline `validates :attr, format: { with: /pattern/ }`.
///   if !(@attr.is_a?(String) && /pattern/.match?(@attr))
///     errors << "attr is invalid"
///   end
/// The runtime helper's `with.nil?` guard is unnecessary — the
/// pattern is a known literal at lower time.
fn inline_format_check(attr: &Symbol, pattern: &str) -> Expr {
    let attr_ivar = ivar(attr);
    let regex_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Regex { pattern: pattern.to_string(), flags: String::new() },
        },
    );
    let is_string = is_a_check(&attr_ivar, "String");
    let match_call = send(regex_lit, "match?", vec![attr_ivar]);
    let valid = bool_op(BoolOpKind::And, is_string, match_call);
    let invalid = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(valid),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err = errors_push(format!("{} is invalid", attr.as_str()));
    if_with_nil_else(invalid, push_err)
}

/// Inline `validates :attr, length: { minimum: M, maximum: N, is: K }`.
/// Produces a single outer `unless @attr.nil?` guard wrapping a Seq:
///   unless @attr.nil?
///     len = if @attr.is_a?(String) then @attr.length elsif @attr.is_a?(Array) then @attr.length else 0 end
///     errors << "attr is too short (minimum is M)" if len < M     # if min set
///     errors << "attr is too long (maximum is N)"  if len > N     # if max set
///     errors << "attr is the wrong length (should be K)" if len != K  # if is set
///   end
/// The `is` (exact length) option isn't in the current ValidationRule
/// shape; left for when the IR adds it.
fn inline_length_check(
    attr: &Symbol,
    min: Option<usize>,
    max: Option<usize>,
    attr_ty: Option<&Ty>,
) -> Vec<Expr> {
    let attr_ivar = ivar(attr);
    // Compute `len`. When the attr's column type is statically known
    // (Str / Array), drop the `is_a?` discrimination — `body : String`
    // can never be an Array, and tsc narrows the dead branch to
    // `never` and rejects the subsequent `.length`. Generic three-way
    // form retained for unknown/dynamic attrs.
    let length_send = send(attr_ivar.clone(), "length", vec![]);
    let len_expr = match attr_ty {
        Some(Ty::Str) | Some(Ty::Array { .. }) => length_send,
        _ => {
            let zero_lit = Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 0 } },
            );
            let inner_else = Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_a_check(&attr_ivar, "Array"),
                    then_branch: length_send.clone(),
                    else_branch: zero_lit,
                },
            );
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_a_check(&attr_ivar, "String"),
                    then_branch: length_send,
                    else_branch: inner_else,
                },
            )
        }
    };
    let len_var = Symbol::from("len");
    let len_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: crate::expr::LValue::Var {
                id: crate::ident::VarId(0),
                name: len_var.clone(),
            },
            value: len_expr,
        },
    );
    let len_read = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: crate::ident::VarId(0), name: len_var },
    );

    let mut inner_stmts: Vec<Expr> = vec![len_assign];
    if let Some(n) = min {
        let lt = send(
            len_read.clone(),
            "<",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: n as i64 } },
            )],
        );
        let msg = format!("{} is too short (minimum is {})", attr.as_str(), n);
        inner_stmts.push(if_with_nil_else(lt, errors_push(msg)));
    }
    if let Some(n) = max {
        let gt = send(
            len_read.clone(),
            ">",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: n as i64 } },
            )],
        );
        let msg = format!("{} is too long (maximum is {})", attr.as_str(), n);
        inner_stmts.push(if_with_nil_else(gt, errors_push(msg)));
    }
    let body_seq = seq(inner_stmts);

    // `unless @attr.nil?` → `if !@attr.nil? then body end`.
    let nil_check = send(attr_ivar, "nil?", vec![]);
    let not_nil = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(nil_check),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    vec![if_with_nil_else(not_nil, body_seq)]
}

/// Inline `validates :attr, numericality: { ... }`.
///   if @attr.nil? || !@attr.is_a?(Numeric)
///     errors << "attr is not a number"
///   else
///     errors << "attr must be greater than G" if @attr <= G      # if gt set
///     errors << "attr must be less than L"    if @attr >= L      # if lt set
///     errors << "attr must be an integer"     if !@attr.is_a?(Integer)  # if only_integer
///   end
/// The if/else form keeps subsequent rules on other attrs running:
/// no early `return` from within `def validate`.
fn inline_numericality_check(
    attr: &Symbol,
    only_integer: bool,
    gt: Option<f64>,
    lt: Option<f64>,
) -> Vec<Expr> {
    let attr_ivar = ivar(attr);
    // `@attr.nil? || !@attr.is_a?(Numeric)` — the "not a number" guard.
    let nil_check = send(attr_ivar.clone(), "nil?", vec![]);
    let is_numeric = is_a_check(&attr_ivar, "Numeric");
    let not_numeric = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(is_numeric),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let bad_cond = bool_op(BoolOpKind::Or, nil_check, not_numeric);
    let nan_msg = errors_push(format!("{} is not a number", attr.as_str()));

    // Build the else-branch Seq of per-option checks.
    let mut else_stmts: Vec<Expr> = Vec::new();
    if let Some(n) = gt {
        let le = send(
            attr_ivar.clone(),
            "<=",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Float { value: n } },
            )],
        );
        let msg = format!("{} must be greater than {}", attr.as_str(), format_float(n));
        else_stmts.push(if_with_nil_else(le, errors_push(msg)));
    }
    if let Some(n) = lt {
        let ge = send(
            attr_ivar.clone(),
            ">=",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Float { value: n } },
            )],
        );
        let msg = format!("{} must be less than {}", attr.as_str(), format_float(n));
        else_stmts.push(if_with_nil_else(ge, errors_push(msg)));
    }
    if only_integer {
        let is_int = is_a_check(&attr_ivar, "Integer");
        let not_int = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(is_int),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let msg = format!("{} must be an integer", attr.as_str());
        else_stmts.push(if_with_nil_else(not_int, errors_push(msg)));
    }
    // If the else has no stmts, just use the if-form (the rule has
    // only the implicit "is a number" check). Otherwise wrap in
    // if/else with both branches populated.
    if else_stmts.is_empty() {
        return vec![if_with_nil_else(bad_cond, nan_msg)];
    }
    let else_branch = seq(else_stmts);
    vec![Expr::new(
        Span::synthetic(),
        ExprNode::If { cond: bad_cond, then_branch: nan_msg, else_branch },
    )]
}

/// Float literal formatter that matches Ruby's default `to_s` shape
/// for whole numbers: `5.0` → "5", `0.5` → "0.5". Matches the runtime
/// helper's `#{greater_than}` interpolation output so error messages
/// agree across inline and helper-call paths.
fn format_float(n: f64) -> String {
    if n == n.trunc() {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ── IR helpers ────────────────────────────────────────────────

fn send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn bool_op(op: BoolOpKind, left: Expr, right: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp { op, surface: BoolOpSurface::default(), left, right },
    )
}

fn is_a_check(value: &Expr, class_name: &str) -> Expr {
    let class_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from(class_name)] },
    );
    send(value.clone(), "is_a?", vec![class_const])
}

/// `errors << <msg_expr>` — pushes a String literal onto the `errors`
/// collection. `errors` is reached via implicit-self Send (the same
/// shape every existing validates_*_of helper produces inside the
/// Validations module).
fn errors_push(msg: String) -> Expr {
    let errors_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("errors"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let msg_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: msg } },
    );
    send(errors_call, "<<", vec![msg_lit])
}

fn if_with_nil_else(cond: Expr, then_branch: Expr) -> Expr {
    let nil_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Nil },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::If { cond, then_branch, else_branch: nil_lit },
    )
}

#[cfg(test)]
mod tests {
    //! Unit tests per ValidationRule arm. Replaces the deleted
    //! `runtime/ruby/test/active_record/validations_test.rb` framework
    //! test, which tested the runtime `validates_*_of` helper methods —
    //! a surface no Group 1 target dispatches into any more (Phase 2.5(a)
    //! inlines every validates declaration here).
    //!
    //! Each test calls the inline helper directly and asserts on the
    //! error-message strings the lowered IR emits, since the error text
    //! is the user-visible contract. Structural shape is covered
    //! implicitly: the message-collection walker only reaches strings
    //! that sit at `errors << "..."` positions.
    use super::*;
    use crate::ident::ClassId;

    /// Walk an `Expr` tree and collect every string literal that's the
    /// RHS of an `errors << "..."` send (the canonical error-push shape
    /// emitted by `errors_push`). Used to assert which error messages
    /// the lowerer arms produce.
    fn collect_error_messages(expr: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        walk(expr, &mut out);
        out
    }

    fn walk(expr: &Expr, out: &mut Vec<String>) {
        match expr.node.as_ref() {
            ExprNode::Send { recv, method, args, .. } => {
                if method.as_str() == "<<" {
                    if let Some(r) = recv.as_ref() {
                        if let ExprNode::Send { method: m, recv: None, .. } = r.node.as_ref() {
                            if m.as_str() == "errors" {
                                if let Some(arg) = args.first() {
                                    if let ExprNode::Lit { value: Literal::Str { value } } =
                                        arg.node.as_ref()
                                    {
                                        out.push(value.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(r) = recv {
                    walk(r, out);
                }
                for a in args {
                    walk(a, out);
                }
            }
            ExprNode::BoolOp { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, out);
                walk(then_branch, out);
                walk(else_branch, out);
            }
            ExprNode::Seq { exprs } => {
                for e in exprs {
                    walk(e, out);
                }
            }
            ExprNode::Assign { value, .. } => walk(value, out),
            _ => {}
        }
    }

    fn attr() -> Symbol {
        Symbol::from("title")
    }

    #[test]
    fn presence_emits_blank_error() {
        let expr = inline_presence_check(&attr(), None);
        assert_eq!(collect_error_messages(&expr), vec!["title can't be blank"]);
    }

    #[test]
    fn presence_str_typed_attr_drops_is_a_array_branch() {
        // Statically-typed string attr: the `is_a?(Array) && ...` arm
        // must drop out, otherwise tsc narrows the dead branch to
        // `never` and rejects the subsequent `.length` access.
        let expr = inline_presence_check(&attr(), Some(&Ty::Str));
        let dbg = format!("{:?}", expr);
        assert!(
            !dbg.contains("\"Array\""),
            "Str-typed attr should drop is_a?(Array); tree: {dbg}",
        );
    }

    #[test]
    fn presence_array_typed_attr_drops_is_a_string_branch() {
        let expr = inline_presence_check(&attr(), Some(&Ty::Array { elem: Box::new(Ty::Str) }));
        let dbg = format!("{:?}", expr);
        assert!(
            !dbg.contains("\"String\""),
            "Array-typed attr should drop is_a?(String); tree: {dbg}",
        );
    }

    #[test]
    fn absence_emits_must_be_blank_error() {
        let expr = inline_absence_check(&attr());
        assert_eq!(collect_error_messages(&expr), vec!["title must be blank"]);
    }

    #[test]
    fn length_min_only_emits_too_short() {
        let exprs = inline_length_check(&attr(), Some(5), None, None);
        assert_eq!(exprs.len(), 1, "length lowers to one outer expression");
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(msgs, vec!["title is too short (minimum is 5)"]);
    }

    #[test]
    fn length_max_only_emits_too_long() {
        let exprs = inline_length_check(&attr(), None, Some(100), None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(msgs, vec!["title is too long (maximum is 100)"]);
    }

    #[test]
    fn length_min_and_max_emits_both_in_order() {
        let exprs = inline_length_check(&attr(), Some(5), Some(100), None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(
            msgs,
            vec![
                "title is too short (minimum is 5)",
                "title is too long (maximum is 100)",
            ],
        );
    }

    #[test]
    fn format_emits_invalid_error() {
        let expr = inline_format_check(&attr(), "[A-Z]+");
        assert_eq!(collect_error_messages(&expr), vec!["title is invalid"]);
    }

    #[test]
    fn inclusion_emits_not_included_error() {
        let values = vec![
            Literal::Str { value: "a".into() },
            Literal::Str { value: "b".into() },
        ];
        let expr = inline_inclusion_check(&attr(), &values);
        assert_eq!(
            collect_error_messages(&expr),
            vec!["title is not included in the list"],
        );
    }

    #[test]
    fn numericality_bare_emits_nan_only() {
        let exprs = inline_numericality_check(&attr(), false, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(msgs, vec!["title is not a number"]);
    }

    #[test]
    fn numericality_with_gt_lt_and_only_integer_emits_all_messages() {
        let exprs = inline_numericality_check(&attr(), true, Some(0.0), Some(100.0));
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        // Order matches the source order in inline_numericality_check:
        // nan-msg (then-branch), then gt → lt → only_integer in else.
        assert_eq!(
            msgs,
            vec![
                "title is not a number",
                "title must be greater than 0",
                "title must be less than 100",
                "title must be an integer",
            ],
        );
    }

    #[test]
    fn numericality_float_bounds_format_without_trailing_zero() {
        // `format_float` matches Ruby's default `to_s` shape for whole
        // numbers: 5.0 → "5", 0.5 → "0.5". Lock the contract so error
        // messages stay byte-stable across cruby and transpiled targets.
        let exprs = inline_numericality_check(&attr(), false, Some(0.5), None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert!(
            msgs.iter().any(|m| m == "title must be greater than 0.5"),
            "expected decimal preserved; got {msgs:?}",
        );
    }

    #[test]
    fn belongs_to_inline_emits_must_exist_error() {
        let assoc_name = Symbol::from("article");
        let foreign_key = Symbol::from("article_id");
        let target = ClassId(Symbol::from("Article"));
        let expr = inline_belongs_to_check(&assoc_name, &foreign_key, &target);
        assert_eq!(collect_error_messages(&expr), vec!["article must exist"]);
    }
}
