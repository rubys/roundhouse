//! Ruby AST → Roundhouse `Expr` — the recursive-descent ingester for
//! expression nodes, shared by every ingest submodule that needs to
//! pull a Ruby body (methods, actions, scopes, seeds, views, tests,
//! and model/controller "Unknown" fallbacks).

use ruby_prism::{Node, parse};

use crate::Symbol;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, InterpPart, Literal};
use crate::span::Span;

use super::util::{
    array_style_from, constant_id_str, constant_path_segments, slice_has_blank_line, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_expr(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    let span = Span::synthetic(); // Real spans land when miette is wired in.
    let expr_node = match node {
        n if n.as_constant_read_node().is_some() => {
            let c = n.as_constant_read_node().unwrap();
            ExprNode::Const {
                path: vec![Symbol::from(constant_id_str(&c.name()))],
            }
        }
        n if n.as_constant_path_node().is_some() => {
            let p = n.as_constant_path_node().unwrap();
            ExprNode::Const { path: constant_path_segments(&p) }
        }
        n if n.as_call_node().is_some() => {
            let c = n.as_call_node().unwrap();
            let method = constant_id_str(&c.name()).to_string();
            let args: Vec<Expr> = if let Some(a) = c.arguments() {
                a.arguments()
                    .iter()
                    .map(|arg| ingest_expr(&arg, file))
                    .collect::<IngestResult<_>>()?
            } else {
                vec![]
            };
            let recv = match c.receiver() {
                Some(r) => Some(ingest_expr(&r, file)?),
                None => None,
            };
            let parenthesized = c.opening_loc().is_some();
            let block = match c.block() {
                Some(block_node) => ingest_call_block(&block_node, file)?,
                None => None,
            };
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block,
                parenthesized,
            }
        }
        n if n.as_integer_node().is_some() => {
            let i = n.as_integer_node().unwrap();
            let v: i32 = i.value().try_into().unwrap_or(0);
            ExprNode::Lit { value: Literal::Int { value: v as i64 } }
        }
        n if n.as_string_node().is_some() => {
            let s = n.as_string_node().unwrap();
            let bytes = s.unescaped();
            ExprNode::Lit {
                value: Literal::Str { value: String::from_utf8_lossy(bytes).into_owned() },
            }
        }
        n if n.as_interpolated_string_node().is_some() => {
            let is = n.as_interpolated_string_node().unwrap();
            let mut parts: Vec<InterpPart> = Vec::new();
            for part in is.parts().iter() {
                if let Some(sn) = part.as_string_node() {
                    let bytes = sn.unescaped();
                    parts.push(InterpPart::Text {
                        value: String::from_utf8_lossy(bytes).into_owned(),
                    });
                } else if let Some(es) = part.as_embedded_statements_node() {
                    let stmts = es.statements().ok_or_else(|| IngestError::Unsupported {
                        file: file.into(),
                        message: "empty `#{}` in interpolated string".into(),
                    })?;
                    let inner = ingest_expr(&stmts.as_node(), file)?;
                    parts.push(InterpPart::Expr { expr: inner });
                } else {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "unsupported interpolated-string part: {part:?}"
                        ),
                    });
                }
            }
            ExprNode::StringInterp { parts }
        }
        n if n.as_symbol_node().is_some() => {
            ExprNode::Lit { value: Literal::Sym { value: symbol_value(n).unwrap_or_default().into() } }
        }
        n if n.as_true_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: true } },
        n if n.as_false_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: false } },
        n if n.as_nil_node().is_some() => ExprNode::Lit { value: Literal::Nil },
        n if n.as_statements_node().is_some() => {
            let stmts = n.as_statements_node().unwrap();
            // The StatementsNode's own location slice is the source for
            // all its children — its bytes let us detect blank-line
            // separators between consecutive stmts without threading the
            // whole source string through every ingest call.
            let block_loc = stmts.location();
            let block_start = block_loc.start_offset();
            let block_bytes = block_loc.as_slice();

            let body_nodes: Vec<Node<'_>> = stmts.body().iter().collect();

            // Guard-clause rewrite: if the first child is
            // `if COND; return; end` followed by more statements,
            // rewrite the whole block as:
            //   if COND then nil else <rest> end
            // Semantically equivalent to the guard (skip rest when
            // COND is true), and keeps the IR free of a bare
            // `return` node which not every target can lower.
            // Triggered by the `return if Article.count > 0`
            // idiom in `db/seeds.rb` (Rails convention for
            // idempotent seed scripts).
            if body_nodes.len() >= 2 {
                if let Some(guard_cond_node) = detect_leading_guard(&body_nodes[0]) {
                    let cond = ingest_expr(&guard_cond_node, file)?;
                    let rest_nodes = &body_nodes[1..];
                    let mut rest_exprs: Vec<Expr> = Vec::with_capacity(rest_nodes.len());
                    let mut prev_end: Option<usize> = None;
                    for child in rest_nodes {
                        let child_start = child.location().start_offset();
                        let mut expr = ingest_expr(child, file)?;
                        if let Some(pe) = prev_end {
                            let from = pe - block_start;
                            let to = child_start - block_start;
                            if slice_has_blank_line(block_bytes, from, to) {
                                expr.leading_blank_line = true;
                            }
                        }
                        rest_exprs.push(expr);
                        prev_end = Some(child.location().end_offset());
                    }
                    let else_branch = if rest_exprs.len() == 1 {
                        rest_exprs.into_iter().next().unwrap()
                    } else {
                        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: rest_exprs })
                    };
                    let nil_expr = Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Nil },
                    );
                    return Ok(Expr::new(
                        Span::synthetic(),
                        ExprNode::If {
                            cond,
                            then_branch: nil_expr,
                            else_branch,
                        },
                    ));
                }
            }

            let mut exprs: Vec<Expr> = Vec::with_capacity(body_nodes.len());
            let mut prev_end: Option<usize> = None;
            for child in &body_nodes {
                let child_start = child.location().start_offset();
                let mut expr = ingest_expr(child, file)?;
                if let Some(pe) = prev_end {
                    let from = pe - block_start;
                    let to = child_start - block_start;
                    if slice_has_blank_line(block_bytes, from, to) {
                        expr.leading_blank_line = true;
                    }
                }
                exprs.push(expr);
                prev_end = Some(child.location().end_offset());
            }
            if exprs.len() == 1 {
                return Ok(exprs.into_iter().next().unwrap());
            }
            ExprNode::Seq { exprs }
        }
        n if n.as_local_variable_read_node().is_some() => {
            let v = n.as_local_variable_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(constant_id_str(&v.name())),
            }
        }
        n if n.as_instance_variable_read_node().is_some() => {
            let v = n.as_instance_variable_read_node().unwrap();
            let raw = constant_id_str(&v.name());
            let name = raw.strip_prefix('@').unwrap_or(raw);
            ExprNode::Ivar { name: Symbol::from(name) }
        }
        n if n.as_if_node().is_some() => {
            let if_node = n.as_if_node().unwrap();
            let cond = ingest_expr(&if_node.predicate(), file)?;
            let then_branch = match if_node.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            let else_branch = match if_node.subsequent() {
                Some(sub) => {
                    if let Some(else_node) = sub.as_else_node() {
                        match else_node.statements() {
                            Some(s) => ingest_expr(&s.as_node(), file)?,
                            None => Expr::new(
                                Span::synthetic(),
                                ExprNode::Seq { exprs: vec![] },
                            ),
                        }
                    } else {
                        // elsif — recurse as nested if.
                        ingest_expr(&sub, file)?
                    }
                }
                None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            };
            ExprNode::If { cond, then_branch, else_branch }
        }
        n if n.as_rescue_modifier_node().is_some() => {
            let r = n.as_rescue_modifier_node().unwrap();
            let expr_inner = ingest_expr(&r.expression(), file)?;
            let fallback = ingest_expr(&r.rescue_expression(), file)?;
            ExprNode::RescueModifier { expr: expr_inner, fallback }
        }
        n if n.as_lambda_node().is_some() => {
            let l = n.as_lambda_node().unwrap();
            let params = l
                .parameters()
                .and_then(|p| {
                    p.as_block_parameters_node().and_then(|bpn| bpn.parameters())
                })
                .map(|pn| {
                    pn.requireds()
                        .iter()
                        .filter_map(|req| req.as_required_parameter_node())
                        .map(|rp| Symbol::from(constant_id_str(&rp.name())))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let body = match l.body() {
                Some(b) => ingest_expr(&b, file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            // `->(x) { body }` literals always use brace form (Prism's
            // opening_loc is `{`); `->(x) do body end` exists but isn't
            // idiomatic and doesn't appear in any fixture yet.
            let block_style = block_style_from_opening(l.opening_loc().as_slice());
            ExprNode::Lambda { params, block_param: None, body, block_style }
        }
        n if n.as_yield_node().is_some() => {
            let y = n.as_yield_node().unwrap();
            let args: Vec<Expr> = if let Some(a) = y.arguments() {
                a.arguments()
                    .iter()
                    .map(|arg| ingest_expr(&arg, file))
                    .collect::<IngestResult<_>>()?
            } else {
                vec![]
            };
            ExprNode::Yield { args }
        }
        n if n.as_or_node().is_some() => {
            let o = n.as_or_node().unwrap();
            let left = ingest_expr(&o.left(), file)?;
            let right = ingest_expr(&o.right(), file)?;
            let surface = bool_op_surface(o.operator_loc().as_slice());
            ExprNode::BoolOp { op: BoolOpKind::Or, surface, left, right }
        }
        n if n.as_and_node().is_some() => {
            let a = n.as_and_node().unwrap();
            let left = ingest_expr(&a.left(), file)?;
            let right = ingest_expr(&a.right(), file)?;
            let surface = bool_op_surface(a.operator_loc().as_slice());
            ExprNode::BoolOp { op: BoolOpKind::And, surface, left, right }
        }
        n if n.as_parentheses_node().is_some() => {
            // Parens are surface-only: unwrap to the inner expression.
            // Empty `()` shouldn't appear in well-formed Ruby, but fall back
            // to `nil` if it does rather than panicking.
            let p = n.as_parentheses_node().unwrap();
            return match p.body() {
                Some(inner) => ingest_expr(&inner, file),
                None => Ok(Expr::new(span, ExprNode::Lit { value: Literal::Nil })),
            };
        }
        n if n.as_array_node().is_some() => {
            let arr = n.as_array_node().unwrap();
            let style = array_style_from(&arr);
            let elements: Vec<Expr> = arr
                .elements()
                .iter()
                .map(|el| ingest_expr(&el, file))
                .collect::<IngestResult<_>>()?;
            ExprNode::Array { elements, style }
        }
        n if n.as_hash_node().is_some() => {
            let hn = n.as_hash_node().unwrap();
            ExprNode::Hash {
                entries: hash_entries_from(&hn.elements(), file)?,
                braced: true,
            }
        }
        n if n.as_keyword_hash_node().is_some() => {
            // Bare keyword args `foo(a: 1)` arrive here when the arg list
            // is passed through generic expression ingest. No braces in source.
            let kh = n.as_keyword_hash_node().unwrap();
            ExprNode::Hash {
                entries: hash_entries_from(&kh.elements(), file)?,
                braced: false,
            }
        }
        n if n.as_instance_variable_write_node().is_some() => {
            let w = n.as_instance_variable_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = raw.strip_prefix('@').unwrap_or(raw);
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: Symbol::from(name) },
                value,
            }
        }
        n if n.as_local_variable_write_node().is_some() => {
            let w = n.as_local_variable_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value,
            }
        }
        // `unless cond; then; else alt; end` lowers to `if cond; alt; else then; end`
        // — same IR, swapped branches. Ruby's semantics match exactly.
        n if n.as_unless_node().is_some() => {
            let u = n.as_unless_node().unwrap();
            let cond = ingest_expr(&u.predicate(), file)?;
            // In Prism, `unless`'s `statements()` is the "when false" body
            // and `consequent()` (if present) is the `else` body.
            let when_false = match u.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            let when_true = match u.else_clause() {
                Some(else_node) => match else_node.statements() {
                    Some(s) => ingest_expr(&s.as_node(), file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                },
                None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            };
            ExprNode::If {
                cond,
                then_branch: when_true,
                else_branch: when_false,
            }
        }
        n if n.as_self_node().is_some() => ExprNode::SelfRef,
        n if n.as_return_node().is_some() => {
            let r = n.as_return_node().unwrap();
            // `return` with no value is `return nil` semantically.
            let value = match r.arguments() {
                Some(a) => {
                    let args: Vec<Node<'_>> = a.arguments().iter().collect();
                    match args.len() {
                        0 => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                        1 => ingest_expr(&args[0], file)?,
                        _ => {
                            // `return a, b` → return an Array (Ruby semantics).
                            let elems = args
                                .iter()
                                .map(|a| ingest_expr(a, file))
                                .collect::<IngestResult<Vec<_>>>()?;
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Array {
                                    elements: elems,
                                    style: crate::expr::ArrayStyle::Brackets,
                                },
                            )
                        }
                    }
                }
                None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            };
            ExprNode::Return { value }
        }
        n if n.as_forwarding_super_node().is_some() => {
            // `super` without parens forwards the current method's args.
            ExprNode::Super { args: None }
        }
        n if n.as_super_node().is_some() => {
            // `super(args)` / `super()` — args = Some(vec).
            let s = n.as_super_node().unwrap();
            let args = match s.arguments() {
                Some(a) => a
                    .arguments()
                    .iter()
                    .map(|arg| ingest_expr(&arg, file))
                    .collect::<IngestResult<Vec<_>>>()?,
                None => vec![],
            };
            ExprNode::Super { args: Some(args) }
        }
        n if n.as_begin_node().is_some() => {
            let b = n.as_begin_node().unwrap();
            let body = match b.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            let mut rescues: Vec<crate::expr::RescueClause> = Vec::new();
            // Walk rescue chain via the parser's `subsequent()` link.
            // Prism doesn't derive Clone on these node wrappers, so we
            // descend by rebinding instead of cloning.
            if let Some(rc) = b.rescue_clause() {
                let mut current_rc = rc;
                loop {
                    let classes = current_rc
                        .exceptions()
                        .iter()
                        .map(|e| ingest_expr(&e, file))
                        .collect::<IngestResult<Vec<_>>>()?;
                    let binding = current_rc.reference().and_then(|r| {
                        r.as_local_variable_target_node()
                            .map(|lvt| Symbol::from(constant_id_str(&lvt.name())))
                    });
                    let rc_body = match current_rc.statements() {
                        Some(s) => ingest_expr(&s.as_node(), file)?,
                        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                    };
                    rescues.push(crate::expr::RescueClause {
                        classes,
                        binding,
                        body: rc_body,
                    });
                    match current_rc.subsequent() {
                        Some(next) => current_rc = next,
                        None => break,
                    }
                }
            }
            let else_branch = match b.else_clause() {
                Some(e) => match e.statements() {
                    Some(s) => Some(ingest_expr(&s.as_node(), file)?),
                    None => None,
                },
                None => None,
            };
            let ensure = match b.ensure_clause() {
                Some(e) => match e.statements() {
                    Some(s) => Some(ingest_expr(&s.as_node(), file)?),
                    None => None,
                },
                None => None,
            };
            ExprNode::BeginRescue {
                body,
                rescues,
                else_branch,
                ensure,
                implicit: false,
            }
        }
        // `@x ||= y` desugars to `@x || (@x = y)` — evaluate `@x`, and only
        // assign on a falsy read. Side-effect-preserving; semantically what
        // Ruby does.
        n if n.as_instance_variable_or_write_node().is_some() => {
            let w = n.as_instance_variable_or_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = raw.strip_prefix('@').unwrap_or(raw).to_string();
            let sym = Symbol::from(name);
            let read = Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: sym.clone() },
            );
            let value = ingest_expr(&w.value(), file)?;
            let assign = Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: sym },
                    value,
                },
            );
            ExprNode::BoolOp {
                op: BoolOpKind::Or,
                surface: BoolOpSurface::Symbol,
                left: read,
                right: assign,
            }
        }
        other => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported expression node: {other:?}"),
            });
        }
    };
    Ok(Expr::new(span, expr_node))
}

/// Parse a Ruby source program (possibly multiple top-level statements)
/// and return the resulting `Expr`. Used by the ERB ingester and by
/// `db/seeds.rb` ingest; generalized so future multi-statement sources
/// can share it.
pub(super) fn ingest_ruby_program(source: &str, file: &str) -> IngestResult<Expr> {
    let result = parse(source.as_bytes());
    let root = result.node();
    let program = root.as_program_node().ok_or_else(|| IngestError::Parse {
        file: file.into(),
        message: "compiled Ruby is not a program".into(),
    })?;
    let stmts = program.statements();
    ingest_expr(&stmts.as_node(), file)
}

/// Guard-clause detector: returns the condition node if `node` is a
/// bare-return guard (`if COND; return; end` with no else, where the
/// then-branch is exactly a valueless `return`). Used by the
/// StatementsNode ingester to rewrite guards into their logical
/// equivalent — `if COND then nil else rest end` — without needing a
/// first-class `Return` IR node. Rails seeds scripts use this idiom
/// (`return if Article.count > 0`) to make seed loading idempotent.
fn detect_leading_guard<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let if_node = node.as_if_node()?;
    // Must have no else branch — otherwise it isn't a guard, it's a
    // regular conditional and the return is one branch's control flow.
    if if_node.subsequent().is_some() {
        return None;
    }
    // Then-branch must be a single bare `return` (no value). Multi-
    // statement then-branches, or returns with values, aren't the
    // guard idiom we're rewriting.
    let then_stmts = if_node.statements()?;
    let then_body: Vec<Node<'_>> = then_stmts.body().iter().collect();
    if then_body.len() != 1 {
        return None;
    }
    let ret = then_body[0].as_return_node()?;
    if ret.arguments().is_some() {
        return None;
    }
    Some(if_node.predicate())
}

/// Ingest a `CallNode`'s block — the `do |...| ... end` or `{ |...| ... }`
/// attached to a method call. Represented as a `Lambda` expression.
/// Returns `None` for block-argument nodes (`&block`) which aren't closures.
fn ingest_call_block(node: &Node<'_>, file: &str) -> IngestResult<Option<Expr>> {
    // `&:method_name` — symbol-to-proc shorthand. Ruby treats this as
    // `{ |x| x.method_name }`. Lower to an explicit Lambda so downstream
    // emitters see a real closure.
    if let Some(ba) = node.as_block_argument_node() {
        if let Some(expr) = ba.expression() {
            if expr.as_symbol_node().is_some() {
                let method_name = symbol_value(&expr).unwrap_or_default();
                let param_name = Symbol::from("x");
                let body = Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            Span::synthetic(),
                            ExprNode::Var {
                                id: crate::ident::VarId(0),
                                name: param_name.clone(),
                            },
                        )),
                        method: Symbol::from(method_name),
                        args: vec![],
                        block: None,
                        // `&:sym` shorthand is a method *call*, not a
                        // property read. Mark as parenthesized so the
                        // emitter produces `x.sym()` not `x.sym`.
                        parenthesized: true,
                    },
                );
                return Ok(Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Lambda {
                        params: vec![param_name],
                        block_param: None,
                        body,
                        block_style: crate::expr::BlockStyle::Brace,
                    },
                )));
            }
            // `&some_proc_var` — passing an existing proc. Not a literal
            // closure; flag as unsupported rather than silently dropping.
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "block-argument forms other than `&:symbol` not yet supported".into(),
            });
        }
        return Ok(None);
    }
    let Some(b) = node.as_block_node() else {
        // Unknown node shape in block position — surface rather than drop.
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unexpected block-position node: {node:?}"),
        });
    };
    let params = block_param_names(&b);
    let body = match b.body() {
        Some(body) => ingest_expr(&body, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    let block_style = block_style_from_opening(b.opening_loc().as_slice());
    Ok(Some(Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { params, block_param: None, body, block_style },
    )))
}

/// Classify a block's `opening_loc` bytes as `{` (brace form) or `do`.
/// Prism always populates this location with the source-literal opener.
fn block_style_from_opening(bytes: &[u8]) -> crate::expr::BlockStyle {
    use crate::expr::BlockStyle;
    if bytes.starts_with(b"{") {
        BlockStyle::Brace
    } else {
        BlockStyle::Do
    }
}

fn block_param_names(b: &ruby_prism::BlockNode<'_>) -> Vec<Symbol> {
    let Some(params_node) = b.parameters() else { return vec![] };
    let Some(bpn) = params_node.as_block_parameters_node() else {
        return vec![];
    };
    let Some(pn) = bpn.parameters() else { return vec![] };
    pn.requireds()
        .iter()
        .filter_map(|req| req.as_required_parameter_node())
        .map(|rp| Symbol::from(constant_id_str(&rp.name())))
        .collect()
}

/// Map the operator bytes of an `OrNode` / `AndNode` to the surface form.
/// Prism's `operator_loc` always points at the actual source bytes, so
/// `&&`/`||` map to `Symbol` and `and`/`or` to `Word`.
fn bool_op_surface(op_bytes: &[u8]) -> BoolOpSurface {
    match op_bytes {
        b"and" | b"or" => BoolOpSurface::Word,
        _ => BoolOpSurface::Symbol,
    }
}

fn hash_entries_from(
    elements: &ruby_prism::NodeList<'_>,
    file: &str,
) -> IngestResult<Vec<(Expr, Expr)>> {
    let mut out = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            // Splats and other non-assoc elements: lift when a fixture demands.
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "non-assoc hash element (splat?) not yet supported".into(),
            });
        };
        let k = ingest_expr(&assoc.key(), file)?;
        let v = ingest_expr(&assoc.value(), file)?;
        out.push((k, v));
    }
    Ok(out)
}
