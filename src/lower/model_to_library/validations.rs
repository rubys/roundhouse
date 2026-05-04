//! Validations: lower `validates :attr, presence: true, length: { ... }` into
//! a single `def validate` body that calls `validates_presence_of(:attr, @attr)`,
//! `validates_length_of(:attr, @attr, minimum: N)` etc. The value is passed
//! as a positional argument (read directly from the matching ivar) rather
//! than yielded through a block — the typed-runtime path. One top-level
//! `def validate` per model; multiple rules across multiple attrs share the
//! same method.

use crate::dialect::{AccessorKind, Association, MethodDef, MethodReceiver, Model, ValidationRule};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

use super::{fn_sig, lit_float, lit_int, lit_sym, seq};

pub(super) fn push_validate_method(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for v in model.validations() {
        for rule in &v.rules {
            stmts.extend(validation_rule_to_calls(&v.attribute, rule));
        }
    }

    // Rails 5+ default: every `belongs_to` requires the associated
    // record to exist before save. Emit `validates_belongs_to(:assoc,
    // @<fk>, <Target>)` per non-optional belongs_to. The runtime
    // helper short-circuits when the FK is unset (nil/0) and queries
    // `<Target>.exists?(fk_value)` otherwise.
    for assoc in model.associations() {
        if let Association::BelongsTo { name, target, foreign_key, optional: false } = assoc {
            stmts.push(belongs_to_validation_call(name, foreign_key, target));
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
    });
}

/// Produce the list of helper-call expressions for one `ValidationRule` on
/// `attr`. Each helper is `<helper>(:attr, @attr [, kwargs])` — the value
/// is passed positionally so the runtime helper sees a concretely-typed
/// `value` parameter (no block-yield, no `instance_variable_get`).
fn validation_rule_to_calls(attr: &Symbol, rule: &ValidationRule) -> Vec<Expr> {
    match rule {
        ValidationRule::Presence => vec![helper_call(
            "validates_presence_of",
            vec![lit_sym(attr.clone()), ivar(attr)],
        )],
        ValidationRule::Absence => vec![helper_call(
            "validates_absence_of",
            vec![lit_sym(attr.clone()), ivar(attr)],
        )],
        ValidationRule::Length { min, max } => {
            let mut entries: Vec<(Expr, Expr)> = Vec::new();
            if let Some(n) = min {
                entries.push((lit_sym(Symbol::from("minimum")), lit_int(*n as i64)));
            }
            if let Some(n) = max {
                entries.push((lit_sym(Symbol::from("maximum")), lit_int(*n as i64)));
            }
            let mut args = vec![lit_sym(attr.clone()), ivar(attr)];
            args.push(Expr::new(
                Span::synthetic(),
                ExprNode::Hash { entries, braced: false },
            ));
            vec![helper_call("validates_length_of", args)]
        }
        ValidationRule::Format { pattern } => vec![helper_call(
            "validates_format_of",
            vec![
                lit_sym(attr.clone()),
                ivar(attr),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash {
                        entries: vec![(
                            lit_sym(Symbol::from("with")),
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit {
                                    value: Literal::Regex {
                                        pattern: pattern.clone(),
                                        flags: String::new(),
                                    },
                                },
                            ),
                        )],
                        braced: false,
                    },
                ),
            ],
        )],
        ValidationRule::Numericality { only_integer, gt, lt } => {
            let mut entries: Vec<(Expr, Expr)> = Vec::new();
            if *only_integer {
                entries.push((
                    lit_sym(Symbol::from("only_integer")),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Bool { value: true } },
                    ),
                ));
            }
            if let Some(n) = gt {
                entries.push((lit_sym(Symbol::from("greater_than")), lit_float(*n)));
            }
            if let Some(n) = lt {
                entries.push((lit_sym(Symbol::from("less_than")), lit_float(*n)));
            }
            let mut args = vec![lit_sym(attr.clone()), ivar(attr)];
            if !entries.is_empty() {
                args.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries, braced: false },
                ));
            }
            vec![helper_call("validates_numericality_of", args)]
        }
        ValidationRule::Inclusion { values } => {
            let array = Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: values
                        .iter()
                        .map(|lit| {
                            Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() })
                        })
                        .collect(),
                    style: ArrayStyle::Brackets,
                },
            );
            let entries = vec![(lit_sym(Symbol::from("in")), array)];
            vec![helper_call(
                "validates_inclusion_of",
                vec![
                    lit_sym(attr.clone()),
                    ivar(attr),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Hash { entries, braced: false },
                    ),
                ],
            )]
        }
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

/// `validates_belongs_to(:<assoc_name>, @<fk>, <TargetClass>)` —
/// emitted once per non-optional `belongs_to`. The third argument is
/// a Const referencing the target model so the helper can dispatch
/// `<Target>.exists?(fk_value)` to verify the FK is live.
fn belongs_to_validation_call(
    assoc_name: &Symbol,
    foreign_key: &Symbol,
    target: &crate::ident::ClassId,
) -> Expr {
    let target_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![target.0.clone()] },
    );
    helper_call(
        "validates_belongs_to",
        vec![lit_sym(assoc_name.clone()), ivar(foreign_key), target_const],
    )
}

/// `@<attr>` — direct ivar read passed as the `value` positional arg
/// to every validates_* helper.
fn ivar(attr: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: attr.clone() })
}
