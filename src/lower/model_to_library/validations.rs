//! Validations: lower `validates :attr, presence: true, length: { ... }` into
//! a single `def validate` body that calls `validates_presence_of(:attr) { @attr }`,
//! `validates_length_of(:attr, minimum: N) { @attr }` etc. — block-yielding
//! shape per the handoff. One top-level `def validate` per model; multiple
//! rules across multiple attrs share the same method.

use crate::dialect::{MethodDef, MethodReceiver, Model, ValidationRule};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use super::{lit_float, lit_int, lit_sym, seq};

pub(super) fn push_validate_method(methods: &mut Vec<MethodDef>, model: &Model) {
    let mut stmts: Vec<Expr> = Vec::new();

    for v in model.validations() {
        for rule in &v.rules {
            stmts.extend(validation_rule_to_calls(&v.attribute, rule));
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
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
    });
}

/// Produce the list of helper-call expressions for one `ValidationRule` on
/// `attr`. Each helper is `<helper>(:attr [, kwargs]) { @attr }` — the
/// block-yielding form is load-bearing per the handoff (must NOT
/// substitute `instance_variable_get`).
fn validation_rule_to_calls(attr: &Symbol, rule: &ValidationRule) -> Vec<Expr> {
    let attr_block = ivar_block(attr);
    match rule {
        ValidationRule::Presence => vec![helper_call(
            "validates_presence_of",
            vec![lit_sym(attr.clone())],
            attr_block,
        )],
        ValidationRule::Absence => vec![helper_call(
            "validates_absence_of",
            vec![lit_sym(attr.clone())],
            attr_block,
        )],
        ValidationRule::Length { min, max } => {
            let mut entries: Vec<(Expr, Expr)> = Vec::new();
            if let Some(n) = min {
                entries.push((lit_sym(Symbol::from("minimum")), lit_int(*n as i64)));
            }
            if let Some(n) = max {
                entries.push((lit_sym(Symbol::from("maximum")), lit_int(*n as i64)));
            }
            let mut args = vec![lit_sym(attr.clone())];
            args.push(Expr::new(
                Span::synthetic(),
                ExprNode::Hash { entries, braced: false },
            ));
            vec![helper_call("validates_length_of", args, attr_block)]
        }
        ValidationRule::Format { pattern } => vec![helper_call(
            "validates_format_of",
            vec![
                lit_sym(attr.clone()),
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
            attr_block,
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
            let mut args = vec![lit_sym(attr.clone())];
            if !entries.is_empty() {
                args.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries, braced: false },
                ));
            }
            vec![helper_call("validates_numericality_of", args, attr_block)]
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
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Hash { entries, braced: false },
                    ),
                ],
                attr_block,
            )]
        }
        ValidationRule::Uniqueness { .. } | ValidationRule::Custom { .. } => {
            // Not yet exercised by real-blog; lands when a fixture forces the issue.
            Vec::new()
        }
    }
}

fn helper_call(name: &str, args: Vec<Expr>, block: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(name),
            args,
            block: Some(block),
            parenthesized: true,
        },
    )
}

/// Produce the `{ @attr }` block lambda used by every validates_* helper.
fn ivar_block(attr: &Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: Vec::new(),
            block_param: None,
            body: Expr::new(Span::synthetic(), ExprNode::Ivar { name: attr.clone() }),
            block_style: crate::expr::BlockStyle::Brace,
        },
    )
}
