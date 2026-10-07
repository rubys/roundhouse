//! `delegated_type :role, types: …, **options` — Rails' polymorphic
//! superclass pattern, expanded at ingest like `enum`.
//!
//! `types:` accepts a literal class list (`%w[]`, `%i[]`,
//! `["Message", …]`, including namespaced `Access::NoticeMessage`) or a
//! compile-time-known constant / concern-constant that resolves to such
//! a list (`TYPES = %w[ Page Section Picture ]`, `types: Leafable::TYPES`).
//! Unresolvable or non-class-name constants fail closed. Documented
//! options (`foreign_key`, `foreign_type`, `primary_key`,
//! `dependent: :destroy`, plus `optional`/`touch`/`default` forwarded
//! to `belongs_to`) are honored; other `dependent:` values and
//! unmodeled belongs_to kwargs (`class_name`, `autosave`, …) stay
//! unexpanded so the unsupported ledger remains honest.

use ruby_prism::Node;

use crate::dialect::{
    AccessorKind, Association, Callback, CallbackHook, Comment, MethodDef, MethodReceiver,
    MethodVisibility, ModelBodyItem, Scope,
};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::naming::{pluralize_snake, underscore};
use crate::span::Span;
use crate::ty::Ty;

use super::util::{bool_value, constant_id_str, string_value, symbol_or_string_value, symbol_value};
use super::IngestResult;

pub(super) fn expand_delegated_type_decl(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    leading_comments: &[Comment],
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<Vec<ModelBodyItem>>> {
    let Some(decl) = parse_declaration(call, file, resolve_constant)? else {
        return Ok(None);
    };
    Ok(Some(expand(decl, leading_comments)))
}

struct Declaration {
    role: Symbol,
    types: Vec<String>,
    foreign_key: Symbol,
    foreign_type: Symbol,
    primary_key: Symbol,
    optional: bool,
    touch: Option<crate::dialect::Touch>,
    default: Option<Expr>,
    dependent_destroy: bool,
    span: Span,
}

fn parse_declaration(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> IngestResult<Option<Declaration>> {
    if call.receiver().is_some() || constant_id_str(&call.name()) != "delegated_type" {
        return Ok(None);
    }
    let Some(args) = call.arguments() else {
        return Ok(None);
    };
    let all: Vec<Node<'_>> = args.arguments().iter().collect();
    let Some(role_node) = all.first() else {
        return Ok(None);
    };
    let Some(role_str) = symbol_value(role_node) else {
        return Ok(None);
    };
    let role = Symbol::from(role_str.as_str());

    let mut types_node: Option<Node<'_>> = None;
    let mut foreign_key = format!("{role}_id");
    let mut foreign_type = format!("{role}_type");
    let mut primary_key = "id".to_string();
    let mut optional = false;
    let mut touch = None;
    let mut default = None;
    let mut dependent = None::<crate::dialect::Dependent>;

    for arg in all.iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else {
            return Ok(None);
        };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else {
                continue;
            };
            let Some(key) = symbol_value(&assoc.key()) else {
                continue;
            };
            let value = assoc.value();
            match key.as_str() {
                "types" => types_node = Some(value),
                "foreign_key" => {
                    let Some(s) = string_value(&value).or_else(|| symbol_value(&value)) else {
                        return Ok(None);
                    };
                    foreign_key = s;
                }
                "foreign_type" => {
                    let Some(s) = string_value(&value).or_else(|| symbol_value(&value)) else {
                        return Ok(None);
                    };
                    foreign_type = s;
                }
                "primary_key" => {
                    let Some(s) = string_value(&value).or_else(|| symbol_value(&value)) else {
                        return Ok(None);
                    };
                    primary_key = s;
                }
                "optional" => {
                    optional = bool_value(&value).unwrap_or(false);
                }
                "touch" => {
                    touch = match bool_value(&value) {
                        Some(true) => Some(crate::dialect::Touch::UpdatedAt),
                        Some(false) => None,
                        None => symbol_value(&value)
                            .map(|s| crate::dialect::Touch::Column(Symbol::from(s.as_str()))),
                    };
                }
                "default" => {
                    default = value
                        .as_lambda_node()
                        .filter(|l| {
                            l.parameters()
                                .and_then(|p| p.as_block_parameters_node().and_then(|b| b.parameters()))
                                .map(|pn| pn.requireds().iter().next().is_none())
                                .unwrap_or(true)
                        })
                        .and_then(|l| l.body())
                        .and_then(|b| super::expr::ingest_expr(&b, file).ok());
                }
                "dependent" => {
                    let Some(s) = symbol_value(&value) else {
                        return Ok(None);
                    };
                    dependent = super::model::dependent_from_sym(&s);
                    if dependent.is_none() {
                        return Ok(None);
                    }
                }
                // Unmodeled belongs_to kwargs (`class_name`, `autosave`,
                // `validate`, …) and unknown keys fail closed: expanding
                // would drop save/type behavior without a ledger entry.
                _ => return Ok(None),
            }
        }
    }

    let Some(types_node) = types_node else {
        return Ok(None);
    };
    let Some(types) = class_type_list(&types_node, resolve_constant) else {
        return Ok(None);
    };
    if types.is_empty() {
        return Ok(None);
    }
    if matches!(
        dependent,
        Some(ref d) if !matches!(d, crate::dialect::Dependent::Destroy | crate::dialect::Dependent::None)
    ) {
        return Ok(None);
    }

    let span = Span {
        file: super::sources::file_id(file),
        start: call.location().start_offset() as u32,
        end: call.location().end_offset() as u32,
    };
    Ok(Some(Declaration {
        role,
        types,
        foreign_key: Symbol::from(foreign_key.as_str()),
        foreign_type: Symbol::from(foreign_type.as_str()),
        primary_key: Symbol::from(primary_key.as_str()),
        optional,
        touch,
        default,
        dependent_destroy: matches!(dependent, Some(crate::dialect::Dependent::Destroy)),
        span,
    }))
}

/// Literal `%w` / `%i` / brackets, or a constant whose compile-time value
/// is that list. Labels must be class names; enum-style `%w[draft …]`
/// constants fail closed rather than inventing predicates.
fn class_type_list(
    node: &Node<'_>,
    resolve_constant: &impl Fn(&Node<'_>) -> Option<Vec<(String, Literal)>>,
) -> Option<Vec<String>> {
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "freeze"
            && call.arguments().is_none()
            && call.block().is_none()
        {
            return class_type_list(&call.receiver()?, resolve_constant);
        }
    }
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() {
        let mapping = resolve_constant(node)?;
        return mapping_as_class_types(&mapping);
    }
    let arr = node.as_array_node()?;
    let mut types = Vec::new();
    for el in arr.elements().iter() {
        let name = symbol_or_string_value(&el)?;
        if !is_class_name(&name) {
            return None;
        }
        types.push(name);
    }
    Some(types)
}

fn mapping_as_class_types(mapping: &[(String, Literal)]) -> Option<Vec<String>> {
    let mut types = Vec::with_capacity(mapping.len());
    for (label, _) in mapping {
        if !is_class_name(label) {
            return None;
        }
        types.push(label.clone());
    }
    Some(types)
}

fn is_class_name(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|seg| {
            !seg.is_empty()
                && seg.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn expand(decl: Declaration, leading_comments: &[Comment]) -> Vec<ModelBodyItem> {
    let span = Span::synthetic();
    let mut items = Vec::new();
    let targets: Vec<ClassId> = decl
        .types
        .iter()
        .map(|t| ClassId(Symbol::from(t.as_str())))
        .collect();
    items.push(ModelBodyItem::Association {
        assoc: Association::BelongsTo {
            name: decl.role.clone(),
            target: ClassId(Symbol::from(crate::naming::camelize(decl.role.as_str()))),
            foreign_key: decl.foreign_key.clone(),
            optional: decl.optional,
            polymorphic: true,
            polymorphic_targets: targets,
            default: decl.default,
            touch: decl.touch,
            foreign_type: Some(decl.foreign_type.clone()),
            primary_key: Some(decl.primary_key.clone()),
        },
        leading_comments: leading_comments.to_vec(),
        leading_blank_line: false,
        span: decl.span,
    });

    items.push(class_method(
        format!("{}_types", decl.role.as_str()),
        Expr::new(
            span,
            ExprNode::Array {
                elements: decl
                    .types
                    .iter()
                    .map(|t| {
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Str { value: t.clone() },
                            },
                        )
                    })
                    .collect(),
                style: ArrayStyle::PercentW,
            },
        ),
    ));

    items.push(instance_method(
        format!("{}_class", decl.role.as_str()),
        class_case(&decl.foreign_type, &decl.types, |t| {
            Expr::new(
                span,
                ExprNode::Const {
                    path: t.split("::").map(Symbol::from).collect(),
                },
            )
        }),
    ));
    items.push(instance_method(
        format!("{}_name", decl.role.as_str()),
        class_case(&decl.foreign_type, &decl.types, |t| {
            Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Str {
                        value: underscore(t).replace('/', "_"),
                    },
                },
            )
        }),
    ));

    for type_name in &decl.types {
        let (scope_name, singular) = delegated_names(type_name);
        items.push(ModelBodyItem::Scope {
            scope: Scope {
                name: Symbol::from(scope_name.as_str()),
                params: Vec::new(),
                body: where_type(&decl.foreign_type, type_name),
            },
            leading_comments: Vec::new(),
            leading_blank_line: false,
        });
        let query = format!("{singular}?");
        items.push(instance_method(
            query.clone(),
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(bare_send(decl.foreign_type.as_str())),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(
                        span,
                        ExprNode::Lit {
                            value: Literal::Str {
                                value: type_name.clone(),
                            },
                        },
                    )],
                    block: None,
                    parenthesized: false,
                },
            ),
        ));
        // `def page; leafable if page?; end` — at the call site the
        // useful type is `Page | nil`, not the full polymorphic
        // `leafable` union. Without that narrowing, `@leaf.page.body`
        // fails on `Section | Picture` arms that do not declare `body`,
        // and Relation/`page` collisions are harder to see honestly.
        let singular_ty = Ty::Union {
            variants: vec![
                Ty::Class {
                    id: ClassId(Symbol::from(type_name.as_str())),
                    args: vec![],
                },
                Ty::Nil,
            ],
        };
        items.push(instance_method_typed(
            singular.clone(),
            if_then(bare_send(&query), bare_send(decl.role.as_str())),
            singular_ty,
        ));
        items.push(instance_method(
            format!("{singular}_{}", decl.primary_key.as_str()),
            if_then(
                bare_send(&query),
                bare_send(decl.foreign_key.as_str()),
            ),
        ));
    }

    if decl.dependent_destroy {
        let destroyer = format!("destroy_{}", decl.role.as_str());
        let reader = bare_send(decl.role.as_str());
        items.push(instance_method(
            destroyer.clone(),
            Expr::new(
                span,
                ExprNode::If {
                    cond: reader.clone(),
                    then_branch: Expr::new(
                        span,
                        ExprNode::Send {
                            recv: Some(reader),
                            method: Symbol::from("destroy"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                    ),
                    else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                },
            ),
        ));
        items.push(ModelBodyItem::Callback {
            callback: Callback {
                hook: CallbackHook::BeforeDestroy,
                targets: vec![Symbol::from(destroyer.as_str())],
                on: None,
                condition: None,
            },
            leading_comments: Vec::new(),
            leading_blank_line: false,
            span: decl.span,
        });
    }
    items
}

fn delegated_names(type_name: &str) -> (String, String) {
    let underscored = underscore(type_name);
    let pluralized = match underscored.rsplit_once('/') {
        Some((head, tail)) => format!("{}/{}", head, pluralize_snake(tail)),
        None => pluralize_snake(&underscored),
    };
    (
        pluralized.replace('/', "_"),
        underscored.replace('/', "_"),
    )
}

fn class_case(type_col: &Symbol, types: &[String], mut body: impl FnMut(&str) -> Expr) -> Expr {
    let span = Span::synthetic();
    let mut arms: Vec<crate::expr::Arm> = types
        .iter()
        .map(|t| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Str { value: t.clone() },
            },
            guard: None,
            body: body(t),
        })
        .collect();
    arms.push(crate::expr::Arm {
        pattern: crate::expr::Pattern::Wildcard,
        guard: None,
        body: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
    });
    Expr::new(
        span,
        ExprNode::Case {
            scrutinee: bare_send(type_col.as_str()),
            arms,
        },
    )
}

fn where_type(type_col: &Symbol, class_name: &str) -> Expr {
    let span = Span::synthetic();
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("where"),
            args: vec![Expr::new(
                span,
                ExprNode::Hash {
                    entries: vec![(
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Sym {
                                    value: type_col.clone(),
                                },
                            },
                        ),
                        Expr::new(
                            span,
                            ExprNode::Lit {
                                value: Literal::Str {
                                    value: class_name.to_string(),
                                },
                            },
                        ),
                    )],
                    kwargs: true,
                },
            )],
            block: None,
            parenthesized: true,
        },
    )
}

fn if_then(cond: Expr, then_branch: Expr) -> Expr {
    let span = Span::synthetic();
    Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch,
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

fn bare_send(method: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn instance_method(name: String, body: Expr) -> ModelBodyItem {
    synth_method(name, MethodReceiver::Instance, body, None)
}

fn instance_method_typed(name: String, body: Expr, ret: Ty) -> ModelBodyItem {
    synth_method(
        name,
        MethodReceiver::Instance,
        body,
        Some(Ty::Fn {
            params: Vec::new(),
            block: None,
            ret: Box::new(ret),
            effects: EffectSet::pure(),
        }),
    )
}

fn class_method(name: String, body: Expr) -> ModelBodyItem {
    synth_method(name, MethodReceiver::Class, body, None)
}

fn synth_method(
    name: String,
    receiver: MethodReceiver,
    body: Expr,
    signature: Option<Ty>,
) -> ModelBodyItem {
    ModelBodyItem::Method {
        method: MethodDef {
            name: Symbol::from(name),
            receiver,
            visibility: MethodVisibility::Public,
            params: Vec::new(),
            unsupported_formals: None,
            has_anonymous_block: false,
            block_param: None,
            name_span: Span::synthetic(),
            body,
            signature,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        },
        leading_comments: Vec::new(),
        leading_blank_line: false,
    }
}
