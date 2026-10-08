//! Ruby AST → Roundhouse `Expr` — the recursive-descent ingester for
//! expression nodes, shared by every ingest submodule that needs to
//! pull a Ruby body (methods, actions, scopes, seeds, views, tests,
//! and model/controller "Unknown" fallbacks).

use ruby_prism::Node;

use crate::Symbol;
use crate::expr::{Arm, BoolOpKind, BoolOpSurface, Expr, ExprNode, InterpPart, Literal, Pattern};
use crate::span::Span;

use super::util::{
    array_style_from, constant_id_str, constant_path_segments, slice_has_blank_line, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_expr(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    // Survey-mode gate: when active, intercept Err returns and
    // substitute a `Literal::Nil` placeholder so the surrounding
    // ingester keeps going. Errors are recorded into the per-thread
    // collector for the post-run punch list. See `survey.rs`.
    match ingest_expr_strict(node, file) {
        Ok(e) => Ok(e),
        Err(err) if super::survey::is_active() => {
            super::survey::record(&err);
            Ok(Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Nil },
            ))
        }
        Err(err) => Err(err),
    }
}


/// Extract one multi-write target — `a`, `@a`, `recv.attr`, or
/// `recv[i]` (e.g. `link['href'], title = attrs`) — as an `LValue`. Shared by the
/// leading targets and the trailing splat target of a `MultiAssign`.
fn multi_write_target(node: &Node<'_>, file: &str) -> IngestResult<crate::expr::LValue> {
    if let Some(lvt) = node.as_local_variable_target_node() {
        Ok(crate::expr::LValue::Var {
            id: crate::ident::VarId(0),
            name: Symbol::from(constant_id_str(&lvt.name())),
        })
    } else if let Some(rp) = node.as_required_parameter_node() {
        // A destructured BLOCK parameter's names (`|(host, secure), i|`)
        // arrive as parameters, not variable targets; they bind locals
        // all the same (`destructure_target`).
        Ok(crate::expr::LValue::Var {
            id: crate::ident::VarId(0),
            name: Symbol::from(constant_id_str(&rp.name())),
        })
    } else if let Some(ivt) = node.as_instance_variable_target_node() {
        let raw = constant_id_str(&ivt.name());
        let name = raw.strip_prefix('@').unwrap_or(raw);
        Ok(crate::expr::LValue::Ivar { name: Symbol::from(name) })
    } else if let Some(ct) = node.as_call_target_node() {
        // `self.a, self.b = pair` / `obj.a, obj.b = pair` — the
        // writer-method target. Prism names the node with the setter's
        // `=` (`a=`); `LValue::Attr` carries the reader name and the
        // emitters append the assignment themselves. `&.` cannot reach
        // here: Ruby rejects it in a multiple-assignment destination.
        let recv = ingest_expr(&ct.receiver(), file)?;
        let raw = constant_id_str(&ct.name());
        let name = raw.strip_suffix('=').unwrap_or(raw);
        Ok(crate::expr::LValue::Attr { recv, name: Symbol::from(name) })
    } else if let Some(it) = node.as_index_target_node() {
        let recv = ingest_expr(&it.receiver(), file)?;
        let index = ingest_index_argument(it.arguments(), file)?;
        Ok(crate::expr::LValue::Index { recv, index })
    } else if let Some(ct) = node.as_constant_target_node() {
        // `A, B = pair` — class-scoped constants, as `A = …` writes.
        Ok(crate::expr::LValue::Const { path: vec![Symbol::from(constant_id_str(&ct.name()))] })
    } else if let Some(cp) = node.as_constant_path_target_node() {
        // `Foo::A, Foo::B = pair` — qualified constants.
        let mut path: Vec<Symbol> = cp
            .parent()
            .and_then(|p| crate::ingest::util::constant_path_segments_strs(&p))
            .unwrap_or_default()
            .into_iter()
            .map(Symbol::from)
            .collect();
        if let Some(id) = cp.name() {
            path.push(Symbol::from(constant_id_str(&id)));
        }
        Ok(crate::expr::LValue::Const { path })
    } else {
        Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported multi-write target: {node:?}"),
        })
    }
}

/// A parenthesized destructuring target — `(b, c)` in `a, (b, c) = x`
/// or in a block's `|(host, secure), index|` — as statements that bind
/// it from `source`: one flat `MultiAssign`, with any target nested
/// further bound through a temp and destructured after it, in order.
/// Ruby's own semantics for each level are the flat multi-write's (the
/// value is splatted with `to_ary`), which every consumer of
/// `MultiAssign` already implements.
///
/// A splat or post-splat target inside the parentheses is refused
/// rather than dropped: the flat rest desugar in `ingest_multi_write`
/// needs an array-literal RHS this source never has.
fn destructure_target(
    mt: &ruby_prism::MultiTargetNode<'_>,
    source: Expr,
    span: Span,
    file: &str,
) -> IngestResult<Vec<Expr>> {
    if mt.rest().is_some() || !mt.rights().is_empty() {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "splat inside a nested destructuring target is not supported".into(),
        });
    }
    let mut targets: Vec<crate::expr::LValue> = Vec::new();
    let mut nested: Vec<Expr> = Vec::new();
    for left in mt.lefts().iter() {
        if let Some(inner) = left.as_multi_target_node() {
            let tmp = nested_temp(&left, span);
            targets.push(crate::expr::LValue::Var { id: crate::ident::VarId(0), name: tmp.clone() });
            let read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: tmp });
            nested.extend(destructure_target(&inner, read, span, file)?);
        } else {
            targets.push(multi_write_target(&left, file)?);
        }
    }
    let mut out = vec![Expr::new(span, ExprNode::MultiAssign { targets, value: source })];
    out.extend(nested);
    Ok(out)
}

/// Ruby writes a nested multi-write's targets depth first, in source
/// order; the desugar above writes one level at a time (the flat level,
/// then each group). The two agree unless a variable is written twice
/// and its LAST write moves — `(x, y), x = [1, 2], 3` leaves `x` at 3 in
/// Ruby and would leave it at 1 here — so that shape is refused rather
/// than answered wrong.
fn reject_reordered_rewrite(mw: &ruby_prism::MultiWriteNode<'_>, file: &str) -> IngestResult<()> {
    fn name_of(node: &Node<'_>) -> Option<String> {
        if let Some(t) = node.as_local_variable_target_node() {
            return Some(constant_id_str(&t.name()).to_string());
        }
        if let Some(t) = node.as_instance_variable_target_node() {
            return Some(constant_id_str(&t.name()).to_string());
        }
        None
    }
    fn source_order<'pr>(
        lefts: impl Iterator<Item = Node<'pr>>,
        writes: &mut Vec<(String, usize)>,
        targets: &mut Vec<usize>,
    ) {
        for left in lefts {
            if let Some(group) = left.as_multi_target_node() {
                source_order(group.lefts().iter(), writes, targets);
            } else {
                let at = left.location().start_offset();
                targets.push(at);
                if let Some(name) = name_of(&left) {
                    writes.push((name, at));
                }
            }
        }
    }
    // The desugar's order: this level's plain targets, then each group's
    // (recursively, in the same shape).
    fn desugar_order<'pr>(
        lefts: Vec<Node<'pr>>,
        writes: &mut Vec<(String, usize)>,
        targets: &mut Vec<usize>,
    ) {
        let mut groups = Vec::new();
        for left in lefts {
            if let Some(group) = left.as_multi_target_node() {
                groups.push(group);
            } else {
                let at = left.location().start_offset();
                targets.push(at);
                if let Some(name) = name_of(&left) {
                    writes.push((name, at));
                }
            }
        }
        for group in groups {
            desugar_order(group.lefts().iter().collect(), writes, targets);
        }
    }
    fn has_setter_target<'pr>(lefts: impl Iterator<Item = Node<'pr>>) -> bool {
        lefts.into_iter().any(|left| {
            if let Some(group) = left.as_multi_target_node() {
                has_setter_target(group.lefts().iter())
            } else {
                left.as_call_target_node().is_some()
            }
        })
    }
    let mut ruby_targets = Vec::new();
    let mut ruby = Vec::new();
    source_order(mw.lefts().iter(), &mut ruby, &mut ruby_targets);
    let mut ours_targets = Vec::new();
    let mut ours = Vec::new();
    desugar_order(mw.lefts().iter().collect(), &mut ours, &mut ours_targets);
    let last = |writes: &[(String, usize)], name: &str| {
        writes.iter().rev().find(|(n, _)| n == name).map(|(_, at)| *at)
    };
    if ruby.iter().any(|(name, _)| last(&ruby, name) != last(&ours, name)) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "nested multi-write that writes one variable twice out of source order is not modeled".into(),
        });
    }
    let has_setter = has_setter_target(mw.lefts().iter());
    if has_setter && ruby_targets != ours_targets {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "nested multi-write with assignment-method targets would reorder setter side effects".into(),
        });
    }
    Ok(())
}

/// A fresh local for one nested destructuring level, unique by the
/// node's offset and clear of every name the source already uses.
fn nested_temp(node: &Node<'_>, _span: Span) -> Symbol {
    let loc = node.location();
    let stem = format!("__mt_{}", loc.start_offset());
    let mut name = stem.clone();
    let mut suffix = 0;
    while super::sources::generated_local_is_reserved(&loc, &name) {
        suffix += 1;
        name = format!("{stem}_{suffix}");
    }
    Symbol::from(name)
}

/// Ingest a `MultiWriteNode` (`a, b = …`, `a, *rest = …`). Split out of
/// the giant `ingest_expr_strict` match so its locals live in a frame
/// entered only for multi-writes, not on every deep recursive descent.
fn ingest_multi_write(
    mw: &ruby_prism::MultiWriteNode<'_>,
    span: Span,
    file: &str,
) -> IngestResult<ExprNode> {
    // `a, (b, c) = x` — a nested target binds a temp here and is
    // destructured from it after the flat assignment. The whole
    // expression still answers the RHS, as Ruby's does.
    if mw.rest().is_none() && mw.lefts().iter().any(|l| l.as_multi_target_node().is_some()) {
        reject_reordered_rewrite(mw, file)?;
        let rhs = nested_temp(&mw.as_node(), span);
        let rhs_read =
            || Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: rhs.clone() });
        let mut exprs = vec![Expr::new(
            span,
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: rhs.clone() },
                value: ingest_expr(&mw.value(), file)?,
            },
        )];
        let mut targets: Vec<crate::expr::LValue> = Vec::new();
        let mut nested: Vec<Expr> = Vec::new();
        for left in mw.lefts().iter() {
            if let Some(inner) = left.as_multi_target_node() {
                let tmp = nested_temp(&left, span);
                targets.push(crate::expr::LValue::Var { id: crate::ident::VarId(0), name: tmp.clone() });
                let read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: tmp });
                nested.extend(destructure_target(&inner, read, span, file)?);
            } else {
                targets.push(multi_write_target(&left, file)?);
            }
        }
        exprs.push(Expr::new(span, ExprNode::MultiAssign { targets, value: rhs_read() }));
        exprs.extend(nested);
        exprs.push(rhs_read());
        return Ok(ExprNode::Seq { exprs });
    }
    let mut targets: Vec<crate::expr::LValue> = Vec::new();
    for left in mw.lefts().iter() {
        targets.push(multi_write_target(&left, file)?);
    }
    let value = ingest_expr(&mw.value(), file)?;
    let rest = match mw.rest() {
        // `a, b = expr` — no splat. Native destructuring node; each
        // target consumes the RHS array positionally.
        None => return Ok(ExprNode::MultiAssign { targets, value }),
        Some(rest) => rest,
    };
    if !mw.rights().is_empty() {
        // The desugar below indexes the RHS directly. Ruby instead calls
        // to_ary (or wraps a scalar); collection dispatch alone does not
        // prove those coercion semantics. Only literal arrays are verified.
        if mw.value().as_array_node().is_none() {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "post-rest multi-write requires an array literal RHS; to_ary coercion is not modeled".into(),
            });
        }
        // Receiver/index expressions on an assignment LHS run BEFORE the
        // RHS in Ruby. This desugar only preserves variable targets; do not
        // accept the new post-rest shape with a different evaluation order.
        for target in mw.lefts().iter().chain(mw.rights().iter())
            .chain(rest.as_splat_node().and_then(|s| s.expression()))
        {
            if target.as_local_variable_target_node().is_none()
                && target.as_instance_variable_target_node().is_none()
            {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "post-rest multi-write with non-variable targets requires preserved LHS evaluation order".into(),
                });
            }
        }
    }
    // `a, *rest = expr` — trailing splat. The shared IR has no rest-aware
    // destructuring node, so desugar to a temp bind + positional `[]`
    // reads + `rest = temp.drop(n)`. Every resulting node (Assign / `[]`
    // / `drop`) is already handled by analyze and all seven emitters, so
    // splat support stays contained to ingest rather than threading a
    // rest flag through ~15 MultiAssign consumers. The Seq ends in the
    // temp read so the whole expression's value is the RHS array —
    // matching Ruby, where `(a, *b = arr)` evaluates to `arr`.
    let stem = format!("__mw_{}", span.start);
    let mut name = stem.clone();
    let mut suffix = 0;
    while super::sources::generated_local_is_reserved(&mw.location(), &name) {
        suffix += 1;
        name = format!("{stem}_{suffix}");
    }
    let tmp = Symbol::from(name);
    let tmp_read = || {
        Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: tmp.clone() })
    };
    let int_lit = |v: i64| {
        Expr::new(span, ExprNode::Lit { value: Literal::Int { value: v } })
    };
    let send = |recv: Expr, method: &str, args: Vec<Expr>| {
        let parenthesized = !args.is_empty() && !matches!(method, "<" | "-");
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized,
            },
        )
    };
    let index_read = |index: Expr| send(tmp_read(), "[]", vec![index]);
    let rights: Vec<Node<'_>> = mw.rights().iter().collect();
    let n_lefts = targets.len();
    let n_rights = rights.len();
    // Leading bindings consume first. A short RHS supplies the remaining
    // values to the right targets from left to right, then pads with nil;
    // negative indexing alone would reuse values already claimed by the head.
    let short_rhs = || send(
        send(tmp_read(), "length", vec![]), "<", vec![int_lit((n_lefts + n_rights) as i64)],
    );
    let mut exprs: Vec<Expr> = Vec::with_capacity(n_lefts + n_rights + 3);
    exprs.push(Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: tmp.clone() },
            value,
        },
    ));
    for (i, target) in targets.into_iter().enumerate() {
        let read = index_read(int_lit(i as i64));
        exprs.push(Expr::new(span, ExprNode::Assign { target, value: read }));
    }
    // Anonymous splat (`a, * = c` / `a, *, c = x`) discards the rest —
    // only a named target gets a binding.
    if let Some(rest_node) = rest.as_splat_node().and_then(|s| s.expression()) {
        let rest_target = multi_write_target(&rest_node, file)?;
        let tail = send(tmp_read(), "drop", vec![int_lit(n_lefts as i64)]);
        let rest_value = if n_rights == 0 {
            tail
        } else {
            let count = Expr::new(
                span,
                ExprNode::If {
                    cond: short_rhs(),
                    then_branch: int_lit(0),
                    else_branch: send(send(tmp_read(), "length", vec![]), "-", vec![int_lit((n_lefts + n_rights) as i64)]),
                },
            );
            send(tail, "take", vec![count])
        };
        exprs.push(Expr::new(span, ExprNode::Assign { target: rest_target, value: rest_value }));
    }
    for (i, right_node) in rights.iter().enumerate() {
        let target = multi_write_target(right_node, file)?;
        let read = Expr::new(span, ExprNode::If {
            cond: short_rhs(),
            then_branch: index_read(int_lit((n_lefts + i) as i64)),
            else_branch: index_read(int_lit(-((n_rights - i) as i64))),
        });
        exprs.push(Expr::new(span, ExprNode::Assign { target, value: read }));
    }
    exprs.push(tmp_read());
    Ok(ExprNode::Seq { exprs })
}

/// The argument a sorbet-runtime assertion evaluates to, when `node`
/// is one: `T.let(x, Type)` / `T.cast` / `T.must` / `T.must_because` /
/// `T.unsafe` / `T.bind` / `T.assert_type!` → `x`. `T.nilable(...)` and
/// friends are type expressions, not values, and are not listed.
pub(super) fn sorbet_assertion_argument<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
    let call = node.as_call_node()?;
    let method = call.name();
    if !matches!(
        constant_id_str(&method),
        "let" | "cast" | "must" | "must_because" | "unsafe" | "bind" | "assert_type!"
    ) {
        return None;
    }
    let receiver = call.receiver()?;
    let constant = receiver.as_constant_read_node()?;
    let name = constant.name();
    if constant_id_str(&name) != "T" {
        return None;
    }
    call.arguments()?.arguments().iter().next()
}

/// Whether the sorbet assertion `node` (see [`sorbet_assertion_argument`])
/// raises on nil, and so answers its argument with nil ruled out.
fn sorbet_assertion_rules_out_nil(node: &Node<'_>) -> bool {
    node.as_call_node()
        .is_some_and(|c| matches!(constant_id_str(&c.name()), "must" | "must_because"))
}

/// The argument of a `T.absurd(x)`, which raises rather than
/// evaluating to it. See the call site in `ingest_expr_strict`.
fn sorbet_absurd_argument<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
    let call = node.as_call_node()?;
    let method = call.name();
    if constant_id_str(&method) != "absurd" {
        return None;
    }
    let constant = call.receiver()?.as_constant_read_node()?;
    if constant_id_str(&constant.name()) != "T" {
        return None;
    }
    call.arguments()?.arguments().iter().next()
}

/// `raise TypeError.new("Control flow reached T.absurd. Got value: " +
/// value.to_s)` — sorbet's own message, including the value, because a
/// bare "unreachable" at the point it is reached says nothing about
/// what got there.
fn absurd_raise(span: Span, value: Expr) -> ExprNode {
    let message = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Lit {
                    value: crate::expr::Literal::Str {
                        value: "Control flow reached T.absurd. Got value: ".to_string(),
                    },
                },
            )),
            method: Symbol::from("+"),
            args: vec![Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(value),
                    method: Symbol::from("to_s"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            )],
            block: None,
            parenthesized: false,
        },
    );
    ExprNode::Send {
        recv: None,
        method: Symbol::from("raise"),
        args: vec![Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("TypeError")] },
                )),
                method: Symbol::from("new"),
                args: vec![message],
                block: None,
                parenthesized: true,
            },
        )],
        block: None,
        parenthesized: true,
    }
}

/// A defined? operand is syntax, not an expression to normalize. In
/// particular, framework predicates and Sorbet assertions must stay calls.
fn ingest_defined_operand(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    if let Some(call) = node.as_call_node() {
        if call.arguments().is_some() || call.block().is_some() || call.is_safe_navigation() {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "defined? calls with arguments, blocks or safe navigation are not modeled".into(),
            });
        }
        let loc = node.location();
        return Ok(Expr::new(
            Span { file: super::sources::file_id(file), start: loc.start_offset() as u32, end: loc.end_offset() as u32 },
            ExprNode::Send {
                recv: call.receiver().map(|recv| ingest_defined_operand(&recv, file)).transpose()?,
                method: Symbol::from(constant_id_str(&call.name())),
                args: vec![],
                block: None,
                parenthesized: call.opening_loc().is_some(),
            },
        ));
    }
    if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some()
        || node.as_forwarding_super_node().is_some() || node.as_local_variable_read_node().is_some()
        || node.as_instance_variable_read_node().is_some() || node.as_class_variable_read_node().is_some()
        || node.as_self_node().is_some()
    {
        return ingest_expr(node, file);
    }
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!("`defined?` does not support this target yet: {node:?}"),
    })
}

/// Whether a trailing `#:` comment after `node` can be a type
/// ascription of the value it evaluates to.
///
/// Writes carry theirs on the assigned value (see the write arms), and
/// definitions have no value to ascribe. A call whose name ends in `=`
/// is an attribute or index write, and `attr_reader :x #: Integer`
/// documents the attribute rather than the call's result.
fn takes_trailing_ascription(node: &Node<'_>) -> bool {
    if let Some(call) = node.as_call_node() {
        let name = call.name();
        let name = constant_id_str(&name);
        let is_write = name.ends_with('=')
            && !matches!(name, "==" | "!=" | "<=" | ">=" | "===");
        let is_attr = call.receiver().is_none()
            && matches!(name, "attr_reader" | "attr_writer" | "attr_accessor");
        return !is_write && !is_attr;
    }
    node.as_parentheses_node().is_some()
        || node.as_local_variable_read_node().is_some()
        || node.as_instance_variable_read_node().is_some()
        || node.as_and_node().is_some()
        || node.as_or_node().is_some()
        || node.as_super_node().is_some()
        || node.as_forwarding_super_node().is_some()
        || node.as_yield_node().is_some()
}

/// Strict expression ingest: never substitutes survey-mode `nil`.
/// Call sites that claim a default or other recovered value use this so
/// a survey recovery cannot be mistaken for a successful parse.
pub(super) fn ingest_expr_strict(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    if !takes_trailing_ascription(node) {
        return ingest_expr_node(node, file);
    }
    // `x || y #: T`: both `y` and the OrNode share the same end offset.
    // Only the outermost eligible expression may consume the comment.
    let end = node.location().end_offset();
    let outermost = super::type_ascription::push_trailing_ascription_claim(end);
    // Drop-guard: ingest paths can panic (`expect` in constant_id_str);
    // a skipped pop would leave a stale end on the thread-local stack
    // and silently drop later trailing `#:` ascriptions at that offset.
    struct PopClaim;
    impl Drop for PopClaim {
        fn drop(&mut self) {
            super::type_ascription::pop_trailing_ascription_claim();
        }
    }
    let expr = {
        let _pop = PopClaim;
        ingest_expr_node(node, file)
    }?;
    if !outermost {
        return Ok(expr);
    }
    Ok(super::type_ascription::ascribe_trailing(
        expr,
        super::type_ascription::trailing_ascription(file, end),
    ))
}

fn ingest_expr_node(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    // Byte offsets into the text registered for `file` (the exact text
    // prism is parsing). FileId(0) when the entry point didn't
    // register — spans then render message-only downstream.
    let loc = node.location();
    let span = Span {
        file: super::sources::file_id(file),
        start: loc.start_offset() as u32,
        end: loc.end_offset() as u32,
    };
    // Type ascriptions retain the value and its declared type. Nil assertions
    // are behavior: until a shared nil-check lowerer exists, retain their
    // explicit Unsupported annotation instead of erasing a possible raise.
    if let Some(inner) = sorbet_assertion_argument(node) {
        let declared = super::type_ascription::sorbet_declared_type(node);
        let value = ingest_expr_strict(&inner, file)?;
        let value = if sorbet_assertion_rules_out_nil(node) {
            super::type_ascription::not_nil(value)
        } else {
            value
        };
        return Ok(super::type_ascription::ascribe(value, declared));
    }

    // `T.absurd(x)` is NOT an assertion that evaluates to its argument:
    // it RAISES. It sits in the branch a case analysis is supposed to
    // have made unreachable, so dropping it would turn "this cannot
    // happen" into "this returns nil" — the one branch where a wrong
    // answer travels furthest before anything notices. Lowered to the
    // raise sorbet performs, message included.
    if let Some(inner) = sorbet_absurd_argument(node) {
        let value = ingest_expr_strict(&inner, file)?;
        return Ok(Expr::new(span, absurd_raise(span, value)));
    }

    let expr_node = match node {
        n if n.as_constant_read_node().is_some() => {
            let c = n.as_constant_read_node().unwrap();
            ExprNode::Const {
                path: vec![Symbol::from(constant_id_str(&c.name()))],
            }
        }
        n if n.as_constant_path_node().is_some() => {
            let p = n.as_constant_path_node().unwrap();
            // `::StatsD` names the top-level constant from anywhere; an empty
            // first segment keeps that through passes that qualify a bare
            // name into its enclosing namespace (`qualify_lexical_consts`
            // made it `BackgroundQueue::StatsD`) and emits as `::StatsD`.
            let mut path = constant_path_segments(&p);
            if super::util::constant_path_is_rooted(&p) {
                path.insert(0, Symbol::from(""));
            }
            ExprNode::Const { path }
        }
        n if n.as_call_node().is_some() => {
            let c = n.as_call_node().unwrap();
            let method = constant_id_str(&c.name()).to_string();
            let args: Vec<Expr> = if let Some(a) = c.arguments() {
                ingest_forwardable_arguments(&a, file)?
            } else {
                vec![]
            };
            let recv = match c.receiver() {
                Some(r) => {
                    let value = ingest_expr(&r, file)?;
                    // `self #: as untyped` above a leading-dot line:
                    // the assertion sits after the receiver and before
                    // the `.method` it is the receiver of.
                    // Receivers that read their own trailing comment
                    // (see `takes_trailing_ascription`) already did.
                    if takes_trailing_ascription(&r) {
                        Some(value)
                    } else {
                        Some(super::type_ascription::ascribe_trailing(
                            value,
                            super::type_ascription::receiver_rbs_assertion(file, r.location().end_offset()),
                        ))
                    }
                }
                None => None,
            };
            let block = match c.block() {
                Some(block_node) => ingest_call_block(&block_node, file, &method)?,
                None => None,
            };
            // Two shapes a paren-less call can't hold once lowered, both
            // of which the emitter can only fix here — by the time it
            // has a rendered `base` string it can't tell where the
            // argument list began:
            //
            //   * a first argument rendering with a leading `{`. The
            //     brace parses as a block. Source can't write that (a
            //     bare hash first arg always carries parens), but the
            //     double-splat desugar produces it: `form_with model: …,
            //     **params` → `form_with({model: …}.merge(params))`.
            //   * a proc-forward block beside arguments. `emit_do_block`
            //     re-attaches `&blk` inside the parens when there are
            //     any, and appends `(&blk)` when there aren't — which
            //     lands after the bare args (`tag.div id: "x"(&__blk)`).
            //     campfire writes `tag.div id: …, data: …, &` all over
            //     its helpers.
            let proc_forward_block = !args.is_empty()
                && block
                    .as_ref()
                    .is_some_and(|b| !matches!(&*b.node, ExprNode::Lambda { .. }));
            let parenthesized = c.opening_loc().is_some()
                || proc_forward_block
                || args.first().is_some_and(starts_with_brace_literal);
            // `+"literal"` — an unfrozen copy of a string literal, the
            // frozen_string_literal-era mutable-builder idiom (spinel
            // e432b19b makes fsl the default; runtime/ruby builders are
            // written `buf = +""`). For every target that transpiles
            // through this IR the frozen/unfrozen distinction doesn't
            // exist — the target's string/builder is whatever it is — so
            // unary `+@` on a string literal is the identity: lower to
            // the literal. The ruby-family trees ship their runtime
            // sources verbatim (never through this path), but an APP's
            // models and services do come through it, and on spinel a
            // bare literal is frozen: `buf = ""; buf << x` raises
            // FrozenError. So the literal carries the hint, and the
            // ruby emitter writes the `+` back.
            if method == "+@" && args.is_empty() && block.is_none() {
                if let Some(r) = &recv {
                    if matches!(
                        &*r.node,
                        ExprNode::Lit { value: Literal::Str { .. } }
                    ) {
                        let mut lit = recv.unwrap();
                        lit.hint = Some(crate::expr::IrHint::MutableStringLiteral);
                        return Ok(lit);
                    }
                }
            }
            // `params.expect(widget: :name)`: Rails wraps a non-Array
            // filter value (`Array.wrap` in `permit_hash`), so it is
            // `params.expect(widget: [:name])`, the spelling every
            // consumer of the expect shape reads.
            let mut args = args;
            if method == "expect"
                && block.is_none()
                && args.len() == 1
                && recv.as_ref().is_some_and(|r| {
                    matches!(&*r.node, ExprNode::Send { recv: None, method, args, block: None, .. }
                        if method.as_str() == "params" && args.is_empty())
                })
            {
                if let ExprNode::Hash { entries, .. } = &mut *args[0].node {
                    for (_, v) in entries.iter_mut() {
                        if matches!(&*v.node, ExprNode::Lit { value: Literal::Sym { .. } }) {
                            let sym = v.clone();
                            *v = Expr::new(sym.span, ExprNode::Array {
                                elements: vec![sym],
                                style: crate::expr::ArrayStyle::Brackets,
                            });
                        }
                    }
                }
            }
            // `binding.local_variable_get(:class)` is how Ruby reads a
            // local named after a reserved word (a keyword param such as
            // `class:`). It is a plain local read, so ingest it as one.
            // The Ruby emitter writes that read back in this form.
            if method == "local_variable_get" && block.is_none() && args.len() == 1 {
                let from_binding = recv.as_ref().is_some_and(|r| {
                    matches!(&*r.node, ExprNode::Send { recv: None, method, args, block: None, .. }
                        if method.as_str() == "binding" && args.is_empty())
                });
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
                    if from_binding && crate::naming::is_reserved_local(value.as_str()) {
                        return Ok(Expr::new(
                            span,
                            ExprNode::Var { id: crate::ident::VarId(0), name: value.clone() },
                        ));
                    }
                }
            }
            // ActiveSupport `recv.try(:sym[, args])` USED TO BE GROUNDED
            // HERE, to `recv && recv.sym(args)`. That is the `&.` shape,
            // and `try` is not `&.`: its definition is
            // `respond_to?(name) && public_send(name, …)`, so it guards
            // DEFINEDNESS and the desugar guarded NILNESS. The two agree
            // exactly when the receiver either is nil or does define the
            // method — which covers most sites and not all, and the one
            // it misses raises where Rails answers nil.
            //
            // Deciding it needs the whole tree (which classes define the
            // name), so it moved to `lower::try_guard`. Ingest leaves the
            // send alone.
            // ActiveSupport `hash.reverse_merge(defaults)` — `defaults`
            // fills in only the keys `hash` lacks (hash's values win). It
            // is exactly `defaults.merge(hash)` in core Ruby, so lower to
            // that (both operands appear once, just swapped). `defaults`
            // is the single hash arg (kwargs collapse to it).
            if method == "reverse_merge"
                && block.is_none()
                && recv.is_some()
                && args.len() == 1
                && !matches!(&*args[0].node, ExprNode::ForwardArgs | ExprNode::ForwardKeywords | ExprNode::ForwardKeywordsWithPairs { .. })
            {
                let r = recv.unwrap();
                let mut defaults = args.into_iter().next().unwrap();
                // The rewritten receiver is the value, not a call-argument
                // keyword marker. Keep the existing reverse-merge semantics.
                if let ExprNode::KeywordSplat { value } = &mut *defaults.node {
                    defaults = std::mem::replace(value, nil_expr());
                }
                // `reverse_merge(a: 1, b: 2)` — the trailing kwargs parsed
                // as a bare (`kwargs: true`) Hash; as the `.merge`
                // RECEIVER it must render braced (`{ a: 1 }.merge(...)`),
                // so re-mark it as a literal hash.
                if let ExprNode::Hash { kwargs, .. } = &mut *defaults.node {
                    *kwargs = false;
                }
                return Ok(Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(defaults),
                        method: Symbol::from("merge"),
                        args: vec![r],
                        block: None,
                        parenthesized: true,
                    },
                ));
            }
            // ActiveRecord `Model.exists?(conditions)` / `rel.exists?(conditions)`
            // — a hash argument is Rails' conditions form, semantically
            // `where(conditions).exists?`. Lower to that chain: `where(hash)`
            // and the zero-arg `Relation#exists?` are both modeled, while a
            // hash-taking `exists?` overload would force is_a?-dispatch into
            // the runtime. The id form (`exists?(5)`) is left for the
            // runtime's `Base.exists?(id)`.
            if method == "exists?"
                && block.is_none()
                && recv.is_some()
                && args.len() == 1
                && match &*args[0].node {
                    ExprNode::Hash { .. } => true,
                    ExprNode::KeywordSplat { value } => matches!(&*value.node, ExprNode::Hash { .. }),
                    _ => false,
                }
            {
                let r = recv.unwrap();
                let mut cond = args.into_iter().next().unwrap();
                if let ExprNode::KeywordSplat { value } = &mut *cond.node {
                    cond = std::mem::replace(value, nil_expr());
                }
                let where_call = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(r),
                        method: Symbol::from("where"),
                        args: vec![cond],
                        block: None,
                        parenthesized: true,
                    },
                );
                return Ok(Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(where_call),
                        method: Symbol::from("exists?"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    },
                ));
            }
            let send = ExprNode::Send {
                recv: recv.clone(),
                method: Symbol::from(method),
                args,
                block,
                parenthesized,
            };
            // Safe navigation `a&.b(args)` — desugar to `a && a.b(args)`
            // (the IR has no safe-send flag). nil receiver → the And
            // yields nil without dispatching, matching `&.`; a plain
            // Send would have silently DROPPED the guard and crashed on
            // nil at runtime. A receiver that is not a plain read is
            // bound once first — `(__safe_nav17 = a.pop) && __safe_nav17.b`
            // — because evaluating it twice is not harmless: campfire's
            // `@idle[address].pop&.first` popped two connections and
            // handed back the wrong one. One divergence stays: a `false`
            // receiver skips the call where Ruby's `&.` would dispatch
            // (nil is the only value `&.` guards) — acceptable until a
            // real call site cares, at which point Send grows a `safe`
            // flag.
            match (c.is_safe_navigation(), recv) {
                (true, Some(r)) if is_plain_read(&r) => ExprNode::BoolOp {
                    op: crate::expr::BoolOpKind::And,
                    surface: crate::expr::BoolOpSurface::Symbol,
                    left: r,
                    right: Expr::new(span, send),
                },
                (true, Some(r)) => {
                    // One local per site: a shared name would hold a
                    // different type at each `&.` in a method, and a
                    // local that does is polymorphic on spinel.
                    let temp = Symbol::from(format!("__safe_nav{}", r.span.start));
                    let read = Expr::new(
                        r.span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: temp.clone() },
                    );
                    let bound = Expr::new(
                        r.span,
                        ExprNode::Assign {
                            target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: temp },
                            value: r,
                        },
                    );
                    let ExprNode::Send { method, args, block, parenthesized, .. } = send else {
                        unreachable!("built as a Send above")
                    };
                    ExprNode::BoolOp {
                        op: crate::expr::BoolOpKind::And,
                        surface: crate::expr::BoolOpSurface::Symbol,
                        left: bound,
                        right: Expr::new(
                            span,
                            ExprNode::Send { recv: Some(read), method, args, block, parenthesized },
                        ),
                    }
                }
                _ => send,
            }
        }
        n if n.as_integer_node().is_some() => {
            let i = n.as_integer_node().unwrap();
            // A literal wider than `i64` is REPORTED, not zeroed. The
            // `unwrap_or(0)` that stood here was silent, and it was
            // reached: `0xffffffff` does not fit an `i32`, so campfire's
            // `ipaddr.to_i & 0xffffffff` emitted `& 0`.
            let Some(v) = super::util::integer_i64(&i.value()) else {
                return Err(IngestError::Unsupported {
                    file: file.to_string(),
                    message: "integer literal does not fit in a 64-bit integer".to_string(),
                });
            };
            ExprNode::Lit { value: Literal::Int { value: v } }
        }
        n if n.as_float_node().is_some() => {
            let f = n.as_float_node().unwrap();
            ExprNode::Lit { value: Literal::Float { value: f.value() } }
        }
        // `999999.9r` / `3r`: an exact Rational literal. Prism gives the
        // value as numerator and denominator (`9999999` and `10`), so it
        // becomes `Rational(9999999, 10)`, the same exact value; going
        // through a Float would round it. `1/3r` is `1 / 3r` and needs
        // nothing else. Kernel#Rational reduces the fraction as the
        // literal does.
        n if n.as_rational_node().is_some() => {
            let r = n.as_rational_node().unwrap();
            let int = |i: &ruby_prism::Integer<'_>| {
                let Some(value) = super::util::integer_i64(i) else {
                    return Err(IngestError::Unsupported {
                        file: file.to_string(),
                        message: "rational literal's numerator or denominator does not fit in a 64-bit integer".to_string(),
                    });
                };
                Ok(Expr::new(span, ExprNode::Lit { value: Literal::Int { value } }))
            };
            ExprNode::Send {
                recv: None,
                method: Symbol::from("Rational"),
                args: vec![int(&r.numerator())?, int(&r.denominator())?],
                block: None,
                parenthesized: true,
            }
        }
        n if n.as_string_node().is_some() => {
            let s = n.as_string_node().unwrap();
            let bytes = s.unescaped();
            match std::str::from_utf8(bytes) {
                Ok(text) => ExprNode::Lit { value: Literal::Str { value: text.to_string() } },
                Err(_) => binary_string(&s, span, file)?,
            }
        }
        n if n.as_interpolated_string_node().is_some() => {
            let is = n.as_interpolated_string_node().unwrap();
            let mut parts: Vec<InterpPart> = Vec::new();
            collect_interp_parts(is.parts(), &mut parts, file)?;
            ExprNode::StringInterp { parts }
        }
        // `:"#{x}_id"` — interpolated symbol. Desugar to the
        // interpolated string sent `.to_sym`; symbols built at
        // runtime are inherently dynamic, same as interp regexes.
        n if n.as_interpolated_symbol_node().is_some() => {
            let is = n.as_interpolated_symbol_node().unwrap();
            let mut parts: Vec<InterpPart> = Vec::new();
            collect_interp_parts(is.parts(), &mut parts, file)?;
            ExprNode::Send {
                recv: Some(Expr::new(Span::synthetic(), ExprNode::StringInterp { parts })),
                method: Symbol::from("to_sym"),
                args: vec![],
                block: None,
                parenthesized: false,
            }
        }
        // `/pattern#{x}flags/` — regex with interpolation. Desugar to
        // `Regexp.new(<interpolated-string>)` so the IR doesn't need
        // a separate RegexInterp variant. The static-only `/foo/`
        // path stays on `Literal::Regex` for round-trip fidelity;
        // interp regexes are inherently runtime constructs anyway.
        //
        // The standard option flags i/m/x are carried through as
        // `Regexp.new`'s second argument (the options bitmask). The
        // `o` (once) flag is dropped: it only memoizes the first
        // interpolation, so re-evaluating is identical for
        // deterministic parts (and merely re-computes otherwise).
        // Encoding flags (e/s/u/n) change match semantics and stay
        // a (rarer) visible gap.
        n if n.as_interpolated_regular_expression_node().is_some() => {
            let r = n.as_interpolated_regular_expression_node().unwrap();
            if r.is_euc_jp()
                || r.is_windows_31j()
                || r.is_utf_8()
                || r.is_ascii_8bit()
            {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "interpolated regex with once/encoding flag not yet supported".into(),
                });
            }
            let mut parts: Vec<InterpPart> = Vec::new();
            collect_interp_parts(r.parts(), &mut parts, file)?;
            let pattern_expr = Expr::new(Span::synthetic(), ExprNode::StringInterp { parts });
            // Ruby Regexp option bits: IGNORECASE=1, EXTENDED=2, MULTILINE=4.
            let opts = (r.is_ignore_case() as i64)
                | ((r.is_extended() as i64) << 1)
                | ((r.is_multi_line() as i64) << 2);
            let mut args = vec![pattern_expr];
            if opts != 0 {
                args.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Int { value: opts } },
                ));
            }
            ExprNode::Send {
                recv: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Regexp")] },
                )),
                method: Symbol::from("new"),
                args,
                block: None,
                parenthesized: true,
            }
        }
        // `defined?(x)` — Ruby keyword (not a method call). Common in
        // Rails view partials to check whether an optional local was
        // passed: `<% if defined?(show_tree_lines) && show_tree_lines %>`.
        //
        // The bareword shape Prism produces for the partial-local idiom
        // — either a no-arg CallNode (when the name isn't lexically
        // bound, which is the partial-local case) or a
        // LocalVariableReadNode (when it IS bound) — lifts to a
        // `Var(name)` reference inside a marker Send; the view-lowerer
        // picks up that inner Var as a partial parameter
        // (collect_extra_params) then rewrites the marker Send to
        // `!name.nil?` (rewrite_defined_to_nil_check). Richer native queries
        // have a dedicated node so no ordinary-call pass rewrites them.
        n if n.as_defined_node().is_some() => {
            let d = n.as_defined_node().unwrap();
            let inner = d.value();
            // These syntax-only answers are known without evaluating an
            // operand or consulting native storage, on every target.
            let descriptor = if inner.as_self_node().is_some() { Some("self") }
                else if inner.as_nil_node().is_some() { Some("nil") }
                else if inner.as_true_node().is_some() { Some("true") }
                else if inner.as_false_node().is_some() { Some("false") }
                else if inner.as_integer_node().is_some() { Some("expression") }
                else { None };
            if let Some(descriptor) = descriptor {
                return Ok(Expr::new(span, ExprNode::Lit {
                    value: Literal::Str { value: descriptor.into() },
                }));
            }
            // Only bareword partial locals need a special representation;
            // native runtime operands use the syntax-only walker.
            let bareword = inner.as_call_node().filter(|c| {
                c.receiver().is_none() && c.arguments().is_none() && c.block().is_none()
            });
            let arg = if let Some(call) = bareword {
                Expr::new(
                    span,
                    ExprNode::Var {
                        id: crate::ident::VarId(0),
                        name: Symbol::from(constant_id_str(&call.name())),
                    },
                )
            } else if inner.as_local_variable_read_node().is_some()
                || inner.as_instance_variable_read_node().is_some()
            {
                ingest_expr(&inner, file)?
            } else if inner.as_call_node().is_some() || inner.as_constant_read_node().is_some()
                || inner.as_constant_path_node().is_some() || inner.as_forwarding_super_node().is_some()
                || inner.as_class_variable_read_node().is_some()
            {
                return Ok(Expr::new(span, ExprNode::Defined {
                    operand: ingest_defined_operand(&inner, file)?,
                }));
            } else {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: format!("`defined?` does not support this target yet: {inner:?}"),
                });
            };
            ExprNode::Send {
                recv: None,
                method: Symbol::from("defined?"),
                args: vec![arg],
                block: None,
                parenthesized: true,
            }
        }
        n if n.as_symbol_node().is_some() => {
            ExprNode::Lit { value: Literal::Sym { value: symbol_value(n).unwrap_or_default().into() } }
        }
        n if n.as_true_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: true } },
        n if n.as_false_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: false } },
        n if n.as_nil_node().is_some() => ExprNode::Lit { value: Literal::Nil },
        // `__FILE__` — Ruby's magic constant for the current source
        // file. Every target needs SOME literal here (Spinel included),
        // and the path is a static fact the ingest already knows (`file`
        // is exactly this file's identity), so there's no reason to
        // treat it as a runtime-only construct. Rendered relative to the
        // app root rather than verbatim (`file` is often an absolute
        // filesystem path built from wherever the app was ingested from)
        // so the literal doesn't bake a local machine's path into the
        // emitted program. A read of `__FILE__` types as `Str` like any
        // other string literal, so `File.expand_path('..', __FILE__)`
        // and friends type-check through it for free.
        n if n.as_source_file_node().is_some() => {
            ExprNode::Lit { value: Literal::Str { value: super::sources::relative_path(file) } }
        }
        // `__LINE__` uses this parse's bytes, not an older registration of
        // the same filename. ERB later translates it to the template line.
        n if n.as_source_line_node().is_some() => {
            let location = n.location();
            let line = super::sources::line_at_parse(&location)
                .or_else(|| super::sources::line_at(file, location.start_offset())).unwrap_or(1);
            ExprNode::Lit { value: Literal::Int { value: line as i64 } }
        }
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
        // Ruby 3.4 `it` implicit block parameter — reads desugar to a
        // plain local named `it`; block_param_names synthesizes the
        // matching |it| parameter from the block's ItParametersNode.
        n if n.as_it_local_variable_read_node().is_some() => {
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("it") }
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
            refuse_block_capture(l.parameters(), file)?;
            let extra_params = block_extra_params(l.parameters(), file)?;
            let params = block_param_names(l.parameters());
            let mut rest_param = block_rest_param(l.parameters());
            let body = match l.body() {
                Some(b) => ingest_expr(&b, file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            let body = destructure_block_params(l.parameters(), body, file)?;
            let body = desugar_post_params(&mut rest_param, block_post_params(l.parameters()), body);
            // `->(x) { body }` literals always use brace form (Prism's
            // opening_loc is `{`); `->(x) do body end` exists but isn't
            // idiomatic and doesn't appear in any fixture yet.
            let block_style = block_style_from_opening(l.opening_loc().as_slice());
            ExprNode::Lambda { extra_params, rest_param, params, block_param: None, body, block_style }
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
            return ingest_hash_literal(&hn.elements(), false, span, file);
        }
        n if n.as_keyword_hash_node().is_some() => {
            // Bare keyword args `foo(a: 1)` arrive here when the arg list
            // is passed through generic expression ingest. No braces in source.
            let kh = n.as_keyword_hash_node().unwrap();
            return ingest_hash_literal(&kh.elements(), true, span, file);
        }
        n if n.as_instance_variable_write_node().is_some() => {
            let w = n.as_instance_variable_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = raw.strip_prefix('@').unwrap_or(raw);
            let value = ingest_expr(&w.value(), file)?;
            let value = super::type_ascription::ascribe_trailing(value, super::type_ascription::trailing_ascription(file, loc.end_offset()));
            ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: Symbol::from(name) },
                value,
            }
        }
        n if n.as_local_variable_write_node().is_some() => {
            let w = n.as_local_variable_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            let value = super::type_ascription::ascribe_trailing(value, super::type_ascription::trailing_ascription(file, loc.end_offset()));
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value,
            }
        }
        // `@@x = y` in a method body (as opposed to the class-body
        // initializer `library_class.rs` handles separately). Mirrors
        // the local-variable-write arm just above rather than the ivar
        // one: like `n.as_class_variable_read_node()` below, the
        // `@@`-prefixed name rides straight into `LValue::Var` so the
        // sigil round-trips on emit (`LValue::Var { name, .. } =>
        // name.to_string()` in `emit/ruby/expr.rs`) without a dedicated
        // class-variable target — we don't model class-variable storage
        // any more richly than that.
        n if n.as_class_variable_write_node().is_some() => {
            let w = n.as_class_variable_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value,
            }
        }
        // `$stdout = STDOUT` — a global-variable write. Like a read
        // (`n.as_global_variable_read_node()` below) and like `@@x`, the
        // `$`-prefixed name rides in `LValue::Var` so the sigil
        // round-trips on the Ruby emit, and the targets without Ruby's
        // globals refuse it by name (`project.rs::report_native_ruby_syntax`).
        n if n.as_global_variable_write_node().is_some() => {
            let w = n.as_global_variable_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value,
            }
        }
        // `FOO = expr` — bare constant write. In a class body this is
        // a class-scoped constant; at top level it's a global constant.
        // Lowerers/emitters resolve the containing scope.
        n if n.as_constant_write_node().is_some() => {
            let w = n.as_constant_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Const { path: vec![name] },
                value,
            }
        }
        // `Foo::BAR = expr` — qualified constant write.
        n if n.as_constant_path_write_node().is_some() => {
            let w = n.as_constant_path_write_node().unwrap();
            let target_node = w.target();
            let path = crate::ingest::util::constant_path_segments(&target_node);
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Const { path },
                value,
            }
        }
        // ── Compound-assignment forms — `target op= value`. ──
        //
        // Six target shapes × three op categories (Or, And, Operator).
        // Each lowers to `ExprNode::OpAssign { target, op, value }`,
        // preserving short-circuit semantics for `||=` / `&&=`. See
        // `OpAssignOp` for the per-target emit story.

        // `x ||= y` — local var, short-circuit.
        n if n.as_local_variable_or_write_node().is_some() => {
            let w = n.as_local_variable_or_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        // `@@x ||= y` — class var, short-circuit (the class-level
        // memoization idiom, same shape as the local-var case above —
        // see the plain `@@x = y` arm for why `LValue::Var` and not
        // `LValue::Ivar` carries the `@@` sigil).
        n if n.as_class_variable_or_write_node().is_some() => {
            let w = n.as_class_variable_or_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_class_variable_and_write_node().is_some() => {
            let w = n.as_class_variable_and_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_class_variable_operator_write_node().is_some() => {
            let w = n.as_class_variable_operator_write_node().unwrap();
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!("unsupported compound-assignment operator: {}", constant_id_str(&w.binary_operator())),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var {
                    id: crate::ident::VarId(0), name: Symbol::from(constant_id_str(&w.name())),
                },
                op,
                value,
            }
        }
        // `$x ||= y`, `$x &&= y`, `$x += y` — a global, as `$x = y` above.
        n if n.as_global_variable_or_write_node().is_some() => {
            let w = n.as_global_variable_or_write_node().unwrap();
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from(constant_id_str(&w.name())) },
                op: crate::expr::OpAssignOp::OrOr,
                value: ingest_expr(&w.value(), file)?,
            }
        }
        n if n.as_global_variable_and_write_node().is_some() => {
            let w = n.as_global_variable_and_write_node().unwrap();
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from(constant_id_str(&w.name())) },
                op: crate::expr::OpAssignOp::AndAnd,
                value: ingest_expr(&w.value(), file)?,
            }
        }
        n if n.as_global_variable_operator_write_node().is_some() => {
            let w = n.as_global_variable_operator_write_node().unwrap();
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!("unsupported compound-assignment operator: {}", constant_id_str(&w.binary_operator())),
                })?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from(constant_id_str(&w.name())) },
                op,
                value: ingest_expr(&w.value(), file)?,
            }
        }
        // `x &&= y` — local var, short-circuit.
        n if n.as_local_variable_and_write_node().is_some() => {
            let w = n.as_local_variable_and_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        // `x += y`, `x -= y`, etc. — local var, arithmetic/bitwise.
        n if n.as_local_variable_operator_write_node().is_some() => {
            let w = n.as_local_variable_operator_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                op,
                value,
            }
        }
        // `@x ||= y` — ivar, short-circuit (memoization idiom).
        n if n.as_instance_variable_or_write_node().is_some() => {
            let w = n.as_instance_variable_or_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = Symbol::from(raw.strip_prefix('@').unwrap_or(raw));
            let value = ingest_expr(&w.value(), file)?;
            let value = super::type_ascription::ascribe_trailing(value, super::type_ascription::trailing_ascription(file, loc.end_offset()));
            ExprNode::OpAssign {
                target: crate::expr::LValue::Ivar { name },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_instance_variable_and_write_node().is_some() => {
            let w = n.as_instance_variable_and_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = Symbol::from(raw.strip_prefix('@').unwrap_or(raw));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Ivar { name },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_instance_variable_operator_write_node().is_some() => {
            let w = n.as_instance_variable_operator_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = Symbol::from(raw.strip_prefix('@').unwrap_or(raw));
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Ivar { name },
                op,
                value,
            }
        }
        // `self.x ||= y`, `obj.x ||= y` — attribute, short-circuit.
        // Setter (`x=`) is suppressed when the read returns truthy —
        // critical for Rails dirty-tracking fidelity.
        n if n.as_call_or_write_node().is_some() => {
            let w = n.as_call_or_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "CallOrWriteNode without receiver".into(),
                    });
                }
            };
            let name = Symbol::from(constant_id_str(&w.read_name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Attr { recv, name },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_call_and_write_node().is_some() => {
            let w = n.as_call_and_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "CallAndWriteNode without receiver".into(),
                    });
                }
            };
            let name = Symbol::from(constant_id_str(&w.read_name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Attr { recv, name },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_call_operator_write_node().is_some() => {
            let w = n.as_call_operator_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "CallOperatorWriteNode without receiver".into(),
                    });
                }
            };
            let name = Symbol::from(constant_id_str(&w.read_name()));
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Attr { recv, name },
                op,
                value,
            }
        }
        // `FOO ||= y`, `FOO &&= y`, `FOO += y` — constant compound writes.
        n if n.as_constant_or_write_node().is_some() => {
            let w = n.as_constant_or_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path: vec![name] },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_constant_and_write_node().is_some() => {
            let w = n.as_constant_and_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path: vec![name] },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_constant_operator_write_node().is_some() => {
            let w = n.as_constant_operator_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path: vec![name] },
                op,
                value,
            }
        }
        n if n.as_constant_path_or_write_node().is_some() => {
            let w = n.as_constant_path_or_write_node().unwrap();
            let path = crate::ingest::util::constant_path_segments(&w.target());
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_constant_path_and_write_node().is_some() => {
            let w = n.as_constant_path_and_write_node().unwrap();
            let path = crate::ingest::util::constant_path_segments(&w.target());
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_constant_path_operator_write_node().is_some() => {
            let w = n.as_constant_path_operator_write_node().unwrap();
            let path = crate::ingest::util::constant_path_segments(&w.target());
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Const { path },
                op,
                value,
            }
        }
        // `arr[i] ||= y` — index target, short-circuit. Receiver and
        // index are evaluated once; setter (`[]=`) suppressed on truthy
        // read.
        n if n.as_index_or_write_node().is_some() => {
            let w = n.as_index_or_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "IndexOrWriteNode without receiver".into(),
                    });
                }
            };
            let index = ingest_index_argument(w.arguments(), file)?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Index { recv, index },
                op: crate::expr::OpAssignOp::OrOr,
                value,
            }
        }
        n if n.as_index_and_write_node().is_some() => {
            let w = n.as_index_and_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "IndexAndWriteNode without receiver".into(),
                    });
                }
            };
            let index = ingest_index_argument(w.arguments(), file)?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Index { recv, index },
                op: crate::expr::OpAssignOp::AndAnd,
                value,
            }
        }
        n if n.as_index_operator_write_node().is_some() => {
            let w = n.as_index_operator_write_node().unwrap();
            let recv = match w.receiver() {
                Some(r) => ingest_expr(&r, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "IndexOperatorWriteNode without receiver".into(),
                    });
                }
            };
            let index = ingest_index_argument(w.arguments(), file)?;
            let op = op_assign_op_from_binary(&constant_id_str(&w.binary_operator()))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "unsupported compound-assignment operator: {}",
                        constant_id_str(&w.binary_operator())
                    ),
                })?;
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::OpAssign {
                target: crate::expr::LValue::Index { recv, index },
                op,
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
                Some(a) => ingest_forwardable_arguments(&a, file)?,
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
        // `recv[idx] op= val` (e.g. `@next_id[name] += 1`) — desugar to
        // `recv[idx] = recv[idx] op val`. Re-evaluates the receiver and
        // index expressions twice, mirroring Ruby's surface semantics
        // for in-place ops on indexed targets.
        n if n.as_index_operator_write_node().is_some() => {
            let w = n.as_index_operator_write_node().unwrap();
            let recv_node = w.receiver().ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "index-operator-write without receiver".into(),
            })?;
            let recv = ingest_expr(&recv_node, file)?;
            let args_node = w.arguments().ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "index-operator-write without arguments".into(),
            })?;
            let mut args: Vec<Expr> = Vec::new();
            for a in args_node.arguments().iter() {
                args.push(ingest_expr(&a, file)?);
            }
            let value = ingest_expr(&w.value(), file)?;
            // Operator is e.g. "+=" — strip trailing "=" to get the
            // binary op name the Send dispatch expects ("+", "-", ...).
            let op_full = constant_id_str(&w.binary_operator());
            let op = op_full.strip_suffix('=').unwrap_or(op_full).to_string();

            // Single-index case is the only shape we've seen in real
            // framework code (`@h[k] += 1`); multi-index `[a, b] += v`
            // would need a tuple Index target. Defer until a fixture
            // forces it.
            if args.len() != 1 {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "index-operator-write with {} indices not yet supported",
                        args.len()
                    ),
                });
            }
            let index = args.remove(0);

            let read = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: Symbol::from("[]"),
                    args: vec![index.clone()],
                    block: None,
                    parenthesized: false,
                },
            );
            let combined = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(read),
                    method: Symbol::from(op),
                    args: vec![value],
                    block: None,
                    parenthesized: false,
                },
            );
            ExprNode::Assign {
                target: crate::expr::LValue::Index { recv, index },
                value: combined,
            }
        }
        // `name op= val` (e.g. `sql += " WHERE..."`) — desugar to
        // `name = name op val`. Mirrors the IndexOperatorWriteNode arm
        // above for indexed targets.
        n if n.as_local_variable_operator_write_node().is_some() => {
            let w = n.as_local_variable_operator_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            let op_full = constant_id_str(&w.binary_operator());
            let op = op_full.strip_suffix('=').unwrap_or(op_full).to_string();
            let read = Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() },
            );
            let combined = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(read),
                    method: Symbol::from(op),
                    args: vec![value],
                    block: None,
                    parenthesized: false,
                },
            );
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value: combined,
            }
        }
        // `recv[idx] ||= val` desugars to `recv[idx] || (recv[idx] = val)`.
        // Same shape as `@x ||= y` below, but with an Index target. Re-
        // evaluates the receiver and index; matches Ruby's surface
        // semantics. The fixture (`@h[k] ||= {}`) only uses single-index
        // form; multi-index defers until needed.
        n if n.as_index_or_write_node().is_some() => {
            let w = n.as_index_or_write_node().unwrap();
            let recv_node = w.receiver().ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "index-or-write without receiver".into(),
            })?;
            let recv = ingest_expr(&recv_node, file)?;
            let args_node = w.arguments().ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "index-or-write without arguments".into(),
            })?;
            let mut args: Vec<Expr> = Vec::new();
            for a in args_node.arguments().iter() {
                args.push(ingest_expr(&a, file)?);
            }
            if args.len() != 1 {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "index-or-write with {} indices not yet supported",
                        args.len()
                    ),
                });
            }
            let index = args.remove(0);
            let value = ingest_expr(&w.value(), file)?;
            let read = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: Symbol::from("[]"),
                    args: vec![index.clone()],
                    block: None,
                    parenthesized: false,
                },
            );
            let assign = Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: crate::expr::LValue::Index { recv, index },
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
        // `$1`, `$2`, ... — regex-match group references. Ruby's
        // implicit globals set by `=~` and `String#match`. Ingest as
        // a `Var` whose name encodes the sigil; `$N` is not a valid
        // local-variable name in Ruby so the namespaces don't collide.
        // The Ruby emitter round-trips by reading the name verbatim.
        n if n.as_numbered_reference_read_node().is_some() => {
            let r = n.as_numbered_reference_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(format!("${}", r.number())),
            }
        }
        // `@@config`, `$stdout`, `$~`/`$&` (back-references) — the three
        // remaining special-read forms, handled like `$1` above: ingest
        // each as a `Var` whose name keeps the sigil verbatim. `@@`/`$`
        // prefixes aren't valid local-variable names, so these can't
        // collide with real locals, and the Ruby emitter round-trips by
        // reading the name back. We don't model class-variable / global
        // state, so the read types as `Var` (gradual). Their value is
        // letting support classes that touch these forms (Keybase's
        // `@@config`, Sponge's `$stdout`, Markdowner's `$&`) ingest at
        // all — without this, one such read drops the whole file under
        // the per-file isolation in `ingest_app`, taking every method on
        // the class with it.
        n if n.as_class_variable_read_node().is_some() => {
            let v = n.as_class_variable_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(constant_id_str(&v.name())),
            }
        }
        n if n.as_global_variable_read_node().is_some() => {
            let v = n.as_global_variable_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(constant_id_str(&v.name())),
            }
        }
        n if n.as_back_reference_read_node().is_some() => {
            let v = n.as_back_reference_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(constant_id_str(&v.name())),
            }
        }
        n if n.as_while_node().is_some() => {
            let w = n.as_while_node().unwrap();
            if w.is_begin_modifier() {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "`begin … end while` (do-while) form not yet supported".into(),
                });
            }
            let cond = ingest_expr(&w.predicate(), file)?;
            let body = match w.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            ExprNode::While { cond, body, until_form: false }
        }
        n if n.as_until_node().is_some() => {
            let u = n.as_until_node().unwrap();
            if u.is_begin_modifier() {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "`begin … end until` (do-until) form not yet supported".into(),
                });
            }
            let cond = ingest_expr(&u.predicate(), file)?;
            let body = match u.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            ExprNode::While { cond, body, until_form: true }
        }
        n if n.as_range_node().is_some() => {
            let r = n.as_range_node().unwrap();
            let begin = match r.left() {
                Some(node) => Some(ingest_expr(&node, file)?),
                None => None,
            };
            let end = match r.right() {
                Some(node) => Some(ingest_expr(&node, file)?),
                None => None,
            };
            ExprNode::Range { begin, end, exclusive: r.is_exclude_end() }
        }
        n if n.as_regular_expression_node().is_some() => {
            let r = n.as_regular_expression_node().unwrap();
            let pattern = String::from_utf8_lossy(r.unescaped()).into_owned();
            let mut flags = String::new();
            // Canonical order: imxoesun (matching Ruby's own to_s).
            if r.is_ignore_case() { flags.push('i'); }
            if r.is_multi_line() { flags.push('m'); }
            if r.is_extended() { flags.push('x'); }
            if r.is_once() { flags.push('o'); }
            if r.is_euc_jp() { flags.push('e'); }
            if r.is_windows_31j() { flags.push('s'); }
            if r.is_utf_8() { flags.push('u'); }
            if r.is_ascii_8bit() { flags.push('n'); }
            ExprNode::Lit { value: Literal::Regex { pattern, flags } }
        }
        n if n.as_next_node().is_some() => {
            let nx = n.as_next_node().unwrap();
            // `next` typically has no args; `next value` and `next a, b`
            // are rarer. Multi-arg `next` returns an Array (Ruby semantics).
            let value = match nx.arguments() {
                None => None,
                Some(a) => {
                    let args: Vec<Node<'_>> = a.arguments().iter().collect();
                    match args.len() {
                        0 => None,
                        1 => Some(ingest_expr(&args[0], file)?),
                        _ => {
                            let elems = args
                                .iter()
                                .map(|a| ingest_expr(a, file))
                                .collect::<IngestResult<Vec<_>>>()?;
                            Some(Expr::new(
                                Span::synthetic(),
                                ExprNode::Array {
                                    elements: elems,
                                    style: crate::expr::ArrayStyle::Brackets,
                                },
                            ))
                        }
                    }
                }
            };
            ExprNode::Next { value }
        }
        // `break` / `break value` / `break a, b` — symmetric to Next,
        // but exits the enclosing iterator entirely. Multi-arg `break`
        // wraps into an Array (Ruby semantics).
        n if n.as_break_node().is_some() => {
            let br = n.as_break_node().unwrap();
            let value = match br.arguments() {
                None => None,
                Some(a) => {
                    let args: Vec<Node<'_>> = a.arguments().iter().collect();
                    match args.len() {
                        0 => None,
                        1 => Some(ingest_expr(&args[0], file)?),
                        _ => {
                            let elems = args
                                .iter()
                                .map(|a| ingest_expr(a, file))
                                .collect::<IngestResult<Vec<_>>>()?;
                            Some(Expr::new(
                                Span::synthetic(),
                                ExprNode::Array {
                                    elements: elems,
                                    style: crate::expr::ArrayStyle::Brackets,
                                },
                            ))
                        }
                    }
                }
            };
            ExprNode::Break { value }
        }
        // `retry` / `redo` — value-less divergent jumps. Placement
        // (retry only inside a rescue body, redo inside a block/loop) is
        // already enforced by the parser, so no validation is needed here.
        n if n.as_retry_node().is_some() => ExprNode::Retry,
        n if n.as_redo_node().is_some() => ExprNode::Redo,
        // `*expr` — splat. Valid in argument lists (`foo(*arr)`) and
        // array literals (`[a, *rest, b]`). The caller (Send/Apply/
        // Array ingest) sees `ExprNode::Splat` wrapping the inner
        // expr and decides how to emit it (varargs spread, slice
        // append, etc.).
        n if n.as_splat_node().is_some() => {
            let s = n.as_splat_node().unwrap();
            let value = match s.expression() {
                Some(e) => ingest_expr(&e, file)?,
                None => Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Nil },
                ),
            };
            ExprNode::Splat { value }
        }
        n if n.as_multi_write_node().is_some() => {
            // Handled out-of-line: `ingest_expr_strict` recurses once per
            // expression-nesting level, so its stack frame is on the hot
            // path for deeply-nested sources (big ERB view trees). Keeping
            // the multi-write locals (temp bind, index reads, splat Seq) in
            // their own frame — entered only when a multi-write is actually
            // hit — stops them from inflating every descent frame.
            ingest_multi_write(&n.as_multi_write_node().unwrap(), span, file)?
        }
        n if n.as_case_node().is_some() => {
            // `case scrutinee when :a, :b then body ... [else else_body] end`
            // Each WhenNode contributes one Arm per pattern (multi-pattern
            // when forms expand into multiple Arms sharing the same body
            // — the IR's Arm holds a single Pattern). `else` lowers to a
            // trailing Wildcard arm.
            let case = n.as_case_node().unwrap();
            let scrutinee = match case.predicate() {
                Some(p) => ingest_expr(&p, file)?,
                // Scrutinee-less `case / when <cond> / else / end` — the
                // Ruby idiom for a condition ladder. There is no value to
                // dispatch on, so `Pattern`/`Arm` can't model it; desugar
                // to the equivalent `if / elsif / else` chain instead.
                // Every target already emits `If`, so this costs no
                // emitter work.
                None => return ingest_condition_ladder(&case, span, file),
            };
            let mut arms: Vec<Arm> = Vec::new();
            for cond in case.conditions().iter() {
                let when = cond.as_when_node().ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!("unsupported case condition (expected when): {cond:?}"),
                })?;
                let body = match when.statements() {
                    Some(s) => ingest_expr(&s.as_node(), file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                };
                let patterns = when.conditions();
                for pat_node in patterns.iter() {
                    let pat_expr = ingest_expr(&pat_node, file)?;
                    // Literal patterns fold into `Pattern::Lit` (cheap
                    // emit + typed-target switch coverage). Anything
                    // else — lambdas, ranges, class refs, calls — lifts
                    // to `Pattern::Expr` so the source `pattern ===
                    // scrutinee` dispatch is preserved. Ruby/Crystal
                    // round-trip these natively; typed-target emit
                    // desugars to predicate-call chains.
                    let pattern = match &*pat_expr.node {
                        ExprNode::Lit { value } => Pattern::Lit { value: value.clone() },
                        _ => Pattern::Expr { expr: pat_expr.clone() },
                    };
                    arms.push(Arm { pattern, guard: None, body: body.clone() });
                }
            }
            if let Some(else_clause) = case.else_clause() {
                let body = match else_clause.statements() {
                    Some(s) => ingest_expr(&s.as_node(), file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                };
                arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body });
            }
            ExprNode::Case { scrutinee, arms }
        }
        n if n.as_case_match_node().is_some() => {
            // `case scrutinee; in pat [guard]; body; ... [else …] end`
            // — Ruby 3's structural pattern matching. Each `InNode`
            // contributes one `MatchArm`; `else` becomes `else_body`
            // (not a synthetic wildcard arm — `CaseMatch` has real
            // NoMatchingPatternError semantics when it's absent, so it
            // needs to stay absent through the IR, not get folded into
            // an arm the way `case/when`'s `else` does).
            let case = n.as_case_match_node().unwrap();
            let scrutinee = match case.predicate() {
                Some(p) => ingest_expr(&p, file)?,
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "case/in with no scrutinee (predicate-less case)".to_string(),
                    });
                }
            };
            let mut arms: Vec<crate::expr::MatchArm> = Vec::new();
            for cond in case.conditions().iter() {
                let in_node = cond.as_in_node().ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: format!("unsupported case/in condition (expected in): {cond:?}"),
                })?;
                let body = match in_node.statements() {
                    Some(s) => ingest_expr(&s.as_node(), file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                };
                let (pattern, guard) = ingest_pattern_with_guard(&in_node.pattern(), file)?;
                arms.push(crate::expr::MatchArm { pattern, guard, body });
            }
            let else_body = match case.else_clause() {
                Some(else_clause) => Some(match else_clause.statements() {
                    Some(s) => ingest_expr(&s.as_node(), file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                }),
                None => None,
            };
            ExprNode::CaseMatch { scrutinee, arms, else_body }
        }
        // `value in pattern` — one-line pattern predicate, `true`/
        // `false`, never raises.
        n if n.as_match_predicate_node().is_some() => {
            let m = n.as_match_predicate_node().unwrap();
            let value = ingest_expr(&m.value(), file)?;
            let pattern = ingest_pattern(&m.pattern(), file)?;
            ExprNode::MatchPredicate { value, pattern }
        }
        // `value => pattern` — one-line pattern assertion: binds on
        // match, raises `NoMatchingPatternError` otherwise.
        n if n.as_match_required_node().is_some() => {
            let m = n.as_match_required_node().unwrap();
            let value = ingest_expr(&m.value(), file)?;
            let pattern = ingest_pattern(&m.pattern(), file)?;
            ExprNode::MatchRequired { value, pattern }
        }
        // Ruby 3.1 hash/keyword value omission (`{short_id:}`,
        // `find_by!(short_id:)`) — prism wraps the implied value
        // (a local read or same-named method call, resolved at parse
        // time) in an ImplicitNode; unwrap and ingest that value.
        n if n.as_implicit_node().is_some() => {
            let implicit = n.as_implicit_node().unwrap();
            return ingest_expr(&implicit.value(), file);
        }
        // `def`/`def self.X` at expression position — appears inside
        // `Class.new(Parent) do ... end` blocks (anonymous-class
        // idiom). Roundhouse's IR has no first-class "method def as
        // expression" node; lift it to a no-op so the surrounding
        // statement sequence still ingests. The behavioral fidelity
        // gap (the resulting anonymous class won't carry the
        // overridden methods) surfaces at runtime, not at ingest.
        n if n.as_def_node().is_some() => {
            ExprNode::Lit { value: Literal::Nil }
        }
        // Backticks / `%x{…}` — a shell-out (`Kernel#\``). Not a
        // construct any target can run, and not worth pretending to
        // model; ledgered by name so it stops reading as the generic
        // "unsupported expression node" catch-all.
        n if n.as_x_string_node().is_some() || n.as_interpolated_x_string_node().is_some() => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "shell command (backticks) is not modeled".into(),
            });
        }
        // Prism's error-recovery node: the parser hit something it
        // couldn't make sense of and inserted a placeholder to keep
        // going. There is no real construct here to ingest — ledgered
        // by name rather than falling through to the generic message,
        // which would misleadingly imply a real Ruby node was rejected.
        n if n.as_missing_node().is_some() => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "unparsed fragment (Prism recovery node)".into(),
            });
        }
        // `class`/`module` at expression position — e.g. inside a
        // method or block body (`Class.new { class Foo; end }`,
        // conditionally-defined classes). Unlike the class-BODY
        // constructs `ingest::model`/`ingest::library_class` walk, a
        // class or module built at runtime, mid-method, has no static
        // home in the IR's class-table model — ledgered specifically
        // rather than falling through to the generic message.
        n if n.as_class_node().is_some() || n.as_module_node().is_some() => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "class/module defined inside a method or block (runtime class definition)"
                    .into(),
            });
        }
        // `class << self … end` at expression position (inside an
        // `included do` / `Class.new do` block, where no class-body walk
        // classifies it). Ruby evaluates the body with `self` the
        // singleton class and the block's value is its last statement
        // (nil when empty). The `def`s in it lift to nil exactly as a
        // bare `def` does above; the statements around them keep their
        // own expressions, so a constant or call in the block is not lost.
        n if n.as_singleton_class_node().is_some() => {
            let sc = n.as_singleton_class_node().unwrap();
            match sc.body() {
                Some(body) => return ingest_expr(&body, file),
                None => ExprNode::Lit { value: Literal::Nil },
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

/// Ingest a `case/in` arm's pattern, unwrapping a guard first when the
/// source wrote one. Prism has no dedicated guard field on `InNode` —
/// `in pat if cond` / `in pat unless cond` are represented by wrapping
/// the REAL pattern node inside an `IfNode`/`UnlessNode` whose
/// `predicate` is the guard condition and whose single-statement
/// `statements` holds the pattern. `MatchPredicateNode`/
/// `MatchRequiredNode` never carry a guard, so they call
/// [`ingest_pattern`] directly instead of this wrapper.
fn ingest_pattern_with_guard(
    node: &Node<'_>,
    file: &str,
) -> IngestResult<(crate::expr::MatchPattern, Option<(crate::expr::MatchGuardKind, Expr)>)> {
    use crate::expr::MatchGuardKind;
    fn unwrap_guarded_pattern<'pr>(
        stmts: Option<ruby_prism::StatementsNode<'pr>>,
        file: &str,
    ) -> IngestResult<Node<'pr>> {
        stmts
            .and_then(|s| s.body().iter().next())
            .ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "case/in guard wraps no pattern statement".to_string(),
            })
    }
    if let Some(if_node) = node.as_if_node() {
        let guard_expr = ingest_expr(&if_node.predicate(), file)?;
        let inner = unwrap_guarded_pattern(if_node.statements(), file)?;
        let pattern = ingest_pattern(&inner, file)?;
        return Ok((pattern, Some((MatchGuardKind::If, guard_expr))));
    }
    if let Some(unless_node) = node.as_unless_node() {
        let guard_expr = ingest_expr(&unless_node.predicate(), file)?;
        let inner = unwrap_guarded_pattern(unless_node.statements(), file)?;
        let pattern = ingest_pattern(&inner, file)?;
        return Ok((pattern, Some((MatchGuardKind::Unless, guard_expr))));
    }
    Ok((ingest_pattern(node, file)?, None))
}

/// Ingest one `case/in` pattern node — the guard-unwrapped `InNode`
/// pattern, a nested sub-pattern inside `Array`/`Find`/`Hash`, or
/// `MatchPredicateNode`/`MatchRequiredNode`'s bare `pattern`. Every
/// Prism pattern node kind Ruby 3's grammar can produce is handled
/// explicitly; anything left over falls through to plain
/// [`ingest_expr`] and becomes a `Value` — safe because Ruby's pattern
/// grammar already rejects an arbitrary expression here unless pinned
/// (handled below), so whatever reaches the fallback is a literal,
/// range, regex, or bare constant/class reference, exactly what
/// `case/when`'s own `Pattern::Expr` escape hatch ingests the same way.
/// A node `ingest_expr` itself doesn't know either surfaces as ITS OWN
/// specifically-named `IngestError::Unsupported`, never a generic
/// pattern-shaped one from here.
fn ingest_pattern(node: &Node<'_>, file: &str) -> IngestResult<crate::expr::MatchPattern> {
    use crate::expr::MatchPattern;

    if let Some(p) = node.as_parentheses_node() {
        let inner = p.body().ok_or_else(|| IngestError::Unsupported {
            file: file.into(), message: "empty parenthesized pattern".into(),
        })?;
        return ingest_pattern(&inner, file);
    }
    if node.as_nil_node().is_some() {
        return Ok(MatchPattern::Nil);
    }
    // Bare bind: `in name`. Ruby's pattern grammar never treats a
    // plain identifier as a value test — that's what `^` is for — so
    // this is unconditionally a Bind, never a Value.
    if let Some(lvt) = node.as_local_variable_target_node() {
        return Ok(MatchPattern::Bind { name: Symbol::from(constant_id_str(&lvt.name())) });
    }
    // `in ^name` (also covers `^@name`/`^$name`/`^@@name` — whichever
    // of those `ingest_expr` resolves; only local/instance reads are
    // modeled today, so a pinned global/class-var surfaces as ITS OWN
    // unsupported-expression error from that call, not a pattern one).
    if let Some(p) = node.as_pinned_variable_node() {
        let expr = ingest_expr(&p.variable(), file)?;
        return Ok(MatchPattern::Value { expr });
    }
    // `in ^(expr)`.
    if let Some(p) = node.as_pinned_expression_node() {
        let expr = ingest_expr(&p.expression(), file)?;
        return Ok(MatchPattern::Value { expr });
    }
    // `in p1 | p2 | ... | pn` — Prism's left-associative binary tree
    // flattens into one `Vec`; only `_`-prefixed bindings are legal.
    if node.as_alternation_pattern_node().is_some() {
        let mut alternatives = Vec::new();
        flatten_alternation(node, file, &mut alternatives)?;
        return Ok(MatchPattern::Alt { alternatives });
    }
    // `in pattern => name`.
    if let Some(cap) = node.as_capture_pattern_node() {
        let pattern = Box::new(ingest_pattern(&cap.value(), file)?);
        let name = Symbol::from(constant_id_str(&cap.target().name()));
        return Ok(MatchPattern::Capture { pattern, name });
    }
    // `in [a, b, *rest, c]` / `in Success(page)` / `in Success[a, b]`.
    if let Some(arr) = node.as_array_pattern_node() {
        return ingest_array_pattern(&arr, file);
    }
    // `in [*, x, y, *post]`.
    if let Some(find) = node.as_find_pattern_node() {
        return ingest_find_pattern(&find, file);
    }
    // `in {status: "ok", data:, **rest}` / `in Success(value:)`.
    if let Some(h) = node.as_hash_pattern_node() {
        return ingest_hash_pattern(&h, file);
    }
    let expr = ingest_expr(node, file)?;
    Ok(MatchPattern::Value { expr })
}

/// Flatten Prism's left-associative `AlternationPatternNode` binary
/// tree (`(a | b) | c` for source `a | b | c`) into one flat `Vec` in
/// source order.
fn flatten_alternation(
    node: &Node<'_>,
    file: &str,
    out: &mut Vec<crate::expr::MatchPattern>,
) -> IngestResult<()> {
    if let Some(alt) = node.as_alternation_pattern_node() {
        flatten_alternation(&alt.left(), file, out)?;
        flatten_alternation(&alt.right(), file, out)?;
    } else {
        out.push(ingest_pattern(node, file)?);
    }
    Ok(())
}

/// Ingest an array pattern's `*`/`*name` rest marker — a `SplatNode`
/// (name-optional: `Some` for `*rest`, `None` for a bare `*`) or an
/// `ImplicitRestNode` (always bare, no name — the trailing-comma
/// shape, `in [a, b,]`). Anything else is a defect in the caller: only
/// these two node kinds ever appear in a pattern's rest position.
fn splat_name(node: &Node<'_>, file: &str) -> IngestResult<Option<Symbol>> {
    if let Some(s) = node.as_splat_node() {
        return match s.expression() {
            None => Ok(None),
            Some(e) => {
                let lvt = e.as_local_variable_target_node().ok_or_else(|| {
                    IngestError::Unsupported {
                        file: file.into(),
                        message: format!("unsupported splat target in pattern: {e:?}"),
                    }
                })?;
                Ok(Some(Symbol::from(constant_id_str(&lvt.name()))))
            }
        };
    }
    if node.as_implicit_rest_node().is_some() {
        return Ok(None);
    }
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!("unsupported pattern rest node: {node:?}"),
    })
}

fn ingest_array_pattern(
    arr: &ruby_prism::ArrayPatternNode<'_>,
    file: &str,
) -> IngestResult<crate::expr::MatchPattern> {
    use crate::expr::MatchPattern;
    let constant = match arr.constant() {
        Some(c) => Some(ingest_expr(&c, file)?),
        None => None,
    };
    let pre = arr
        .requireds()
        .iter()
        .map(|n| ingest_pattern(&n, file))
        .collect::<IngestResult<Vec<_>>>()?;
    let post = arr
        .posts()
        .iter()
        .map(|n| ingest_pattern(&n, file))
        .collect::<IngestResult<Vec<_>>>()?;
    let rest = match arr.rest() {
        None => None,
        Some(r) => Some(splat_name(&r, file)?),
    };
    Ok(MatchPattern::Array { constant, pre, rest, post })
}

fn ingest_find_pattern(
    find: &ruby_prism::FindPatternNode<'_>,
    file: &str,
) -> IngestResult<crate::expr::MatchPattern> {
    use crate::expr::MatchPattern;
    let constant = match find.constant() {
        Some(c) => Some(ingest_expr(&c, file)?),
        None => None,
    };
    let pre_rest = splat_name(&find.left().as_node(), file)?;
    let middle = find
        .requireds()
        .iter()
        .map(|n| ingest_pattern(&n, file))
        .collect::<IngestResult<Vec<_>>>()?;
    let post_rest = splat_name(&find.right(), file)?;
    Ok(MatchPattern::Find { constant, pre_rest, middle, post_rest })
}

fn ingest_hash_pattern(
    h: &ruby_prism::HashPatternNode<'_>,
    file: &str,
) -> IngestResult<crate::expr::MatchPattern> {
    use crate::expr::MatchPattern;
    let constant = match h.constant() {
        Some(c) => Some(ingest_expr(&c, file)?),
        None => None,
    };
    let mut pairs = Vec::new();
    for el in h.elements().iter() {
        let assoc = el.as_assoc_node().ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported hash pattern element: {el:?}"),
        })?;
        let key = symbol_value(&assoc.key()).ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported hash pattern key: {:?}", assoc.key()),
        })?;
        let value_node = assoc.value();
        // Ruby 3.1 keyword-value-omission shorthand (`data:`): Prism
        // wraps the implied `LocalVariableTargetNode` in an
        // `ImplicitNode`. That IS what `MatchPattern::Hash`'s `None`
        // pair value means, so unwrap and discard rather than
        // ingesting the wrapped bind as an explicit sub-pattern (which
        // would double-bind under a redundant `Bind` node).
        let sub = if value_node.as_implicit_node().is_some() {
            None
        } else {
            Some(ingest_pattern(&value_node, file)?)
        };
        pairs.push((Symbol::from(key.as_str()), sub));
    }
    let rest = match h.rest() {
        None => None,
        Some(r) => Some(ingest_hash_rest(&r, file)?),
    };
    Ok(MatchPattern::Hash { constant, pairs, rest })
}

fn ingest_hash_rest(node: &Node<'_>, file: &str) -> IngestResult<crate::expr::HashRest> {
    use crate::expr::HashRest;
    if node.as_no_keywords_parameter_node().is_some() {
        return Ok(HashRest::Nil);
    }
    if let Some(splat) = node.as_assoc_splat_node() {
        let Some(value) = splat.value() else { return Ok(HashRest::Ignore); };
        let lvt = value.as_local_variable_target_node().ok_or_else(|| {
            IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported hash pattern **rest target: {value:?}"),
            }
        })?;
        return Ok(HashRest::Collect { name: Symbol::from(constant_id_str(&lvt.name())) });
    }
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!("unsupported hash pattern rest node: {node:?}"),
    })
}

/// Map a Prism `binary_operator` symbol (`+`, `-`, `<<`, …) to the IR
/// `OpAssignOp`. Returns `None` if the operator isn't one we model
/// today — the caller reports `IngestError::Unsupported` so unknown
/// op names surface explicitly rather than silently emitting wrong code.
fn op_assign_op_from_binary(op: &str) -> Option<crate::expr::OpAssignOp> {
    use crate::expr::OpAssignOp;
    match op {
        "+" => Some(OpAssignOp::Add),
        "-" => Some(OpAssignOp::Sub),
        "*" => Some(OpAssignOp::Mul),
        "/" => Some(OpAssignOp::Div),
        "%" => Some(OpAssignOp::Mod),
        "**" => Some(OpAssignOp::Pow),
        "&" => Some(OpAssignOp::BitAnd),
        "|" => Some(OpAssignOp::BitOr),
        "^" => Some(OpAssignOp::BitXor),
        "<<" => Some(OpAssignOp::Shl),
        ">>" => Some(OpAssignOp::Shr),
        _ => None,
    }
}

/// Extract the single index argument from a `[]`-shaped index node's
/// arguments. The compound `arr[i] op= y` Prism nodes share this
/// shape: arguments is `Some(ArgumentsNode)` with exactly one child
/// (the index expression). Multi-dim indexing (`m[i, j]`) is out of
/// scope; we report `Unsupported` if encountered.
/// Collect the parts of an interpolated string (or interpolated
/// regex) into a flat `Vec<InterpPart>`. Recursively flattens any
/// nested `InterpolatedStringNode` parts — Prism represents adjacent
/// string literals (`"foo" "bar"`, including line-continued ones
/// `"foo" \` ↵ `"bar"`) as an outer InterpolatedString with no
/// opening/closing whose parts are themselves inner InterpolatedStrings.
/// The IR has a single `StringInterp { parts }` shape, so the inner
/// parts splice into the outer list.
fn collect_interp_parts(
    parts_node: ruby_prism::NodeList<'_>,
    out: &mut Vec<InterpPart>,
    file: &str,
) -> IngestResult<()> {
    for part in parts_node.iter() {
        if let Some(sn) = part.as_string_node() {
            let bytes = sn.unescaped();
            out.push(InterpPart::Text {
                value: String::from_utf8_lossy(bytes).into_owned(),
            });
        } else if let Some(es) = part.as_embedded_statements_node() {
            let stmts = es.statements().ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "empty `#{}` in interpolated string".into(),
            })?;
            let inner = ingest_expr(&stmts.as_node(), file)?;
            out.push(InterpPart::Expr { expr: inner });
        } else if let Some(nested) = part.as_interpolated_string_node() {
            // Adjacent / line-continued string concatenation — flatten
            // the inner parts into the outer list. The inner's quote
            // delimiters drop out at flatten time; they only mattered
            // for source-level parsing.
            collect_interp_parts(nested.parts(), out, file)?;
        } else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported interpolated-string part: {part:?}"),
            });
        }
    }
    Ok(())
}

fn ingest_index_argument(
    args: Option<ruby_prism::ArgumentsNode<'_>>,
    file: &str,
) -> IngestResult<Expr> {
    let args = args.ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "compound index-write missing argument".into(),
    })?;
    let arg_list = args.arguments();
    let mut iter = arg_list.iter();
    let first = iter.next().ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "compound index-write argument list is empty".into(),
    })?;
    if iter.next().is_some() {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "compound index-write with multi-dim index not yet supported".into(),
        });
    }
    ingest_expr(&first, file)
}

/// Parse a Ruby source program (possibly multiple top-level statements)
/// and return the resulting `Expr`. Used by the ERB ingester and by
/// `db/seeds.rb` ingest; generalized so future multi-statement sources
/// can share it.
pub(super) fn ingest_ruby_program(source: &str, file: &str) -> IngestResult<Expr> {
    super::sources::register(file, source);
    // Silent parse, NOT the parse-diagnostic wrapper: `source` here is the
    // compiled-from-ERB buffer (or a seeds script), parsed out of its
    // true method-body context. Prism flags context-only errors on it —
    // a layout's `<%= yield %>` compiles to a top-level `yield`, which is
    // "Invalid yield" as a standalone program but legitimate in the view
    // method roundhouse ingests it into. Reporting those would be a false
    // positive on every layout, so this path stays silent (as it was
    // before the wrapper); real `.rb` source files report via the wrapper.
    let result = super::prism::parse_silent(source.as_bytes());
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

/// Preserve packet identity for a lone anonymous `**`, or for static symbol
/// key/value pairs followed by one trailing anonymous `**`. The latter keeps
/// pair order and lets the packet override duplicate explicit keys. Earlier
/// or repeated anonymous splats, named/dynamic splats in the same group, and
/// non-static keys remain unsupported because the IR has no ordered dynamic
/// merge form. The argument-list walk is adapted from Tim Tischler's F7 commit
/// 013588ec; retaining the marker avoids a positional hash and synthetic
/// user-visible bindings.
fn ingest_forwardable_arguments(
    a: &ruby_prism::ArgumentsNode<'_>,
    file: &str,
) -> IngestResult<Vec<Expr>> {
    let mut args = Vec::new();
    for arg in a.arguments().iter() {
        if arg.as_forwarding_arguments_node().is_some() {
            let loc = arg.location();
            args.push(Expr::new(Span {
                file: super::sources::file_id(file),
                start: loc.start_offset() as u32,
                end: loc.end_offset() as u32,
            }, ExprNode::ForwardArgs));
        } else {
            if let Some(hash) = arg.as_keyword_hash_node() {
                let elements: Vec<_> = hash.elements().iter().collect();
                let anonymous_splats: Vec<usize> = elements
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| {
                        e.as_assoc_splat_node()
                            .is_some_and(|s| s.value().is_none())
                            .then_some(i)
                    })
                    .collect();
                if !anonymous_splats.is_empty() {
                    if elements.len() == 1 {
                        let loc = elements[0].location();
                        args.push(Expr::new(Span {
                            file: super::sources::file_id(file),
                            start: loc.start_offset() as u32,
                            end: loc.end_offset() as u32,
                        }, ExprNode::ForwardKeywords));
                        continue;
                    }
                    if anonymous_splats.len() != 1
                        || anonymous_splats[0] != elements.len() - 1
                    {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: "anonymous `**` keyword forwarding not yet supported".into(),
                        });
                    }

                    let mut entries = Vec::with_capacity(elements.len() - 1);
                    for element in &elements[..elements.len() - 1] {
                        let Some(assoc) = element.as_assoc_node() else {
                            // In particular, a named `**options` before the
                            // anonymous packet would require a second dynamic
                            // merge in the IR. Keep that mixed shape explicit
                            // on the unsupported ledger until it is modeled.
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "anonymous `**` keyword forwarding not yet supported".into(),
                            });
                        };
                        let key = ingest_expr(&assoc.key(), file)?;
                        if !matches!(
                            &*key.node,
                            ExprNode::Lit { value: Literal::Sym { .. } }
                        ) {
                            return Err(IngestError::Unsupported {
                                file: file.into(),
                                message: "anonymous `**` keyword forwarding requires static symbol keys".into(),
                            });
                        }
                        let value = ingest_expr(&assoc.value(), file)?;
                        entries.push((key, value));
                    }
                    let loc = hash.location();
                    args.push(Expr::new(Span {
                        file: super::sources::file_id(file),
                        start: loc.start_offset() as u32,
                        end: loc.end_offset() as u32,
                    }, ExprNode::ForwardKeywordsWithPairs { entries }));
                    continue;
                }
            }
            let value = ingest_expr(&arg, file)?;
            // Only a CALL's KeywordHashNode owns this fact. `{**h}`
            // remains an ordinary positional hash/merge expression.
            let has_keyword_splat = arg.as_keyword_hash_node().is_some_and(|hash| {
                hash.elements().iter().any(|e| e.as_assoc_splat_node().is_some())
            });
            args.push(if has_keyword_splat {
                Expr::new(value.span, ExprNode::KeywordSplat { value })
            } else {
                value
            });
        }
    }
    Ok(args)
}

/// Ingest a `CallNode`'s block — the `do |...| ... end` or `{ |...| ... }`
/// attached to a method call. Represented as a `Lambda` expression.
/// Returns `None` for block-argument nodes (`&block`) which aren't closures.
fn ingest_call_block(
    node: &Node<'_>,
    file: &str,
    enclosing_method: &str,
) -> IngestResult<Option<Expr>> {
    if matches!(enclosing_method, "find_or_create_by" | "find_or_create_by!")
        && node.as_block_argument_node().is_some()
    {
        // Inlining only claims a directly attached initialization block.
        // Forwarded proc/lambda forms can lose their signature before
        // the ordinary BlockNode admission below sees it.
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "scoped find-or-create cannot be lowered safely: forwarded initialization blocks are outside the directly attached block subset".into(),
        });
    }
    // `&:method_name` — symbol-to-proc shorthand. Ruby treats this as
    // `{ |x| x.method_name }`. Lower to an explicit Lambda so downstream
    // emitters see a real closure.
    if let Some(ba) = node.as_block_argument_node() {
        if let Some(expr) = ba.expression() {
            if expr.as_symbol_node().is_some() {
                let method_name = symbol_value(&expr).unwrap_or_default();
                // Symbol#to_proc is arity-adaptive: for a 1-arg yield
                // (`map(&:name)`) it's `{ |x| x.name }`; for the 2-arg
                // accumulator yield of `inject`/`reduce` (`inject(&:+)`)
                // it's `{ |acc, x| acc.+(x) }` (the symbol names the
                // operator applied to the memo with the element). A fixed
                // 1-param lambda is wrong for the latter — it drops the
                // element and calls `memo.+` with no argument. Pick the
                // shape from the receiving method.
                let two_arg = matches!(enclosing_method, "inject" | "reduce");
                // Anchor the desugared call (and the lambda) at the
                // `:sym` token: diagnostics inside the expansion (e.g.
                // missing_preload's access site) then render a real
                // file:line and span-containment consumers (traceroute
                // hop annotations) can place them. The params stay
                // synthetic — they have no source text.
                let loc = expr.location();
                let sym_span = Span {
                    file: super::sources::file_id(file),
                    start: loc.start_offset() as u32,
                    end: loc.end_offset() as u32,
                };
                let var = |name: &Symbol| {
                    // The symbol's own span, not a synthetic one: this
                    // variable exists only because `&:sym` was
                    // expanded, so a diagnostic about it has nowhere
                    // else to point. Span-less, it printed with no
                    // file and no line at all.
                    Expr::new(
                        sym_span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() },
                    )
                };
                let (params, recv_name, call_args) = if two_arg {
                    let acc = Symbol::from("acc");
                    let x = Symbol::from("x");
                    (vec![acc.clone(), x.clone()], acc, vec![var(&x)])
                } else {
                    let x = Symbol::from("x");
                    (vec![x.clone()], x, vec![])
                };
                let body = Expr::new(
                    sym_span,
                    ExprNode::Send {
                        recv: Some(var(&recv_name)),
                        method: Symbol::from(method_name),
                        args: call_args,
                        block: None,
                        // `&:sym` shorthand is a method *call*, not a
                        // property read. Mark as parenthesized so the
                        // emitter produces `x.sym()` not `x.sym`.
                        parenthesized: true,
                    },
                );
                return Ok(Some(Expr::new(
                    sym_span,
                    ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
                        params,
                        block_param: None,
                        body,
                        block_style: crate::expr::BlockStyle::Brace,
                    },
                )));
            }
            // `&block_var` — forwarding an existing proc bound to a
            // local (the `&block` parameter idiom). Lower to a bare
            // `ExprNode::Var` in the `block:` slot — the slot itself
            // signals Proc-forward (slot context disambiguates Var-as-
            // value vs Var-as-Proc, sidestepping a new IR variant +
            // its ~84-site exhaustive-match sweep). Per-target emit
            // recognizes a non-Lambda block expression as forwarding.
            // Issue #25 stage 2.
            if let Some(v) = expr.as_local_variable_read_node() {
                return Ok(Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: crate::ident::VarId(0),
                        name: Symbol::from(constant_id_str(&v.name())),
                    },
                )));
            }
            // `&method(:name)` / `&self.method(:name)` / `&recv.method(:name)`
            // — a bound-method reference (Procore's dominant non-`&:sym`
            // block-argument shape — `entities.map(&method(:name))`).
            // `method` must be a bare, receiverless-or-not call to
            // Kernel's `method`, taking exactly one Symbol argument;
            // anything else falls through to the other shapes below.
            if let Some(call) = expr.as_call_node() {
                if constant_id_str(&call.name()) == "method" {
                    if let Some(method_ref) = try_ingest_method_ref(&call, file)? {
                        return Ok(Some(method_ref));
                    }
                }
                // `&proc { ... }` / `&lambda { ... }` — Kernel's proc-
                // construction methods. Semantics differ from a plain
                // block only in `return`/arity strictness (a lambda's
                // `return` exits the lambda, not the enclosing method);
                // Roundhouse doesn't yet distinguish that, so desugaring
                // to the same `Lambda` IR a literal block would produce
                // is an approximation, not a mis-compile, for the
                // common no-`return`-inside case.
                if matches!(constant_id_str(&call.name()), "proc" | "lambda")
                    && call.receiver().is_none()
                    && call.arguments().is_none()
                {
                    if let Some(blk) = call.block() {
                        if let Some(b) = blk.as_block_node() {
                            return Ok(Some(ingest_block_node_as_lambda(&b, file)?));
                        }
                    }
                }
            }
            // `&->(params) { body }` — a stabby lambda literal. Same
            // params/body grammar as a plain block (`BlockParametersNode`
            // either way), so it desugars identically; only the
            // standalone-lambda-vs-attached-block distinction is lost,
            // which is immaterial once it sits in block-argument position.
            if let Some(lam) = expr.as_lambda_node() {
                refuse_block_capture(lam.parameters(), file)?;
                let extra_params = block_extra_params(lam.parameters(), file)?;
                let params = block_param_names(lam.parameters());
                let mut rest_param = block_rest_param(lam.parameters());
                let body = match lam.body() {
                    Some(body) => ingest_expr(&body, file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                };
                let body = destructure_block_params(lam.parameters(), body, file)?;
                let body = desugar_post_params(&mut rest_param, block_post_params(lam.parameters()), body);
                let block_style = block_style_from_opening(lam.opening_loc().as_slice());
                return Ok(Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Lambda { extra_params, rest_param, params, block_param: None, body, block_style },
                )));
            }
            // Any other proc-valued expression is evaluated once and passed as the block.
            return Ok(Some(ingest_expr(&expr, file)?));
        }
        // Ruby 3.4 anonymous block forwarding (`fetch(key, &)`) —
        // reference the synthesized `__blk` binding the def-side
        // anonymous `&` param ingests to (see the controller /
        // library-class method ingests).
        return Ok(Some(Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("__blk") },
        )));
    }
    let Some(b) = node.as_block_node() else {
        // Unknown node shape in block position — surface rather than drop.
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unexpected block-position node: {node:?}"),
        });
    };
    if matches!(enclosing_method, "find_or_create_by" | "find_or_create_by!")
        && b.parameters().is_some_and(|node| {
            node.as_block_parameters_node().is_some_and(|params| {
                params.locals().iter().next().is_some()
                    || params.parameters().is_some_and(|params| {
                        params.requireds().iter().any(|node| node.as_required_parameter_node().is_none())
                            || params.optionals().iter().next().is_some()
                            || params.posts().iter().next().is_some()
                            || params.keywords().iter().next().is_some()
                            || params.keyword_rest().is_some()
                            || params.block().is_some()
                            || params.rest().is_some_and(|node| node.as_rest_parameter_node()
                                .is_none_or(|param| param.name().is_none()))
                    })
            })
        })
    {
        // These signature fields are absent from Lambda IR. Decline
        // before ingest drops them; named rest remains represented and
        // is rejected by the call-site lowering's existing arity gate.
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "scoped find-or-create cannot be lowered safely: block locals and optional, post, keyword, block or anonymous-rest parameters are not represented in initialization-block IR".into(),
        });
    }
    Ok(Some(ingest_block_node_as_lambda(&b, file)?))
}

/// Ingest a literal block's params/body into a `Lambda` `Expr`. Shared
/// by the ordinary `foo { ... }`/`foo do ... end` path above and the
/// `&proc { ... }`/`&lambda { ... }` block-argument desugar, which
/// hands this the same `BlockNode` shape one level deeper (inside the
/// `proc`/`lambda` call's own block).
fn ingest_block_node_as_lambda(b: &ruby_prism::BlockNode<'_>, file: &str) -> IngestResult<Expr> {
    refuse_block_capture(b.parameters(), file)?;
    let extra_params = block_extra_params(b.parameters(), file)?;
    let params = block_param_names(b.parameters());
    let mut rest_param = block_rest_param(b.parameters());
    let body = match b.body() {
        Some(body) => ingest_expr(&body, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    let body = destructure_block_params(b.parameters(), body, file)?;
    let body = desugar_post_params(&mut rest_param, block_post_params(b.parameters()), body);
    let block_style = block_style_from_opening(b.opening_loc().as_slice());
    Ok(Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params, rest_param, params, block_param: None, body, block_style },
    ))
}

/// `method(:name)` / `recv.method(:name)` → `ExprNode::MethodRef`.
/// Returns `Ok(None)` when `call`'s shape doesn't match (wrong arg
/// count, non-Symbol argument) so the caller falls through to the
/// other block-argument forms rather than committing to a wrong
/// desugar. `call.name()` (`== "method"`) is checked by the caller.
fn try_ingest_method_ref(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<Expr>> {
    let Some(args_node) = call.arguments() else { return Ok(None) };
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let [arg] = args.as_slice() else { return Ok(None) };
    if arg.as_symbol_node().is_none() {
        return Ok(None);
    }
    let name = symbol_value(arg).unwrap_or_default();
    let recv = match call.receiver() {
        Some(r) => Some(ingest_expr(&r, file)?),
        None => None,
    };
    Ok(Some(Expr::new(
        Span::synthetic(),
        ExprNode::MethodRef { recv, name: Symbol::from(name) },
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

/// Authored block captures are not retained by the Lambda ingest below.
/// Refuse before dropping a binding that could shadow an outer local.
fn refuse_block_capture(params_node: Option<Node<'_>>, file: &str) -> IngestResult<()> {
    if params_node.and_then(|node| node.as_block_parameters_node())
        .and_then(|node| node.parameters()).is_some_and(|params| params.block().is_some())
    {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "block parameter capture requires preserved Lambda signature semantics".into(),
        });
    }
    Ok(())
}

/// The block's optional positionals, keywords and keyword rest, in source
/// order, for `ExprNode::Lambda.extra_params`. `params` holds only the
/// required names, and the body reads these as well: dropping them made
/// the emitted lambda raise `NameError`. `**nil` (no keywords accepted)
/// has no representation here and is refused: dropped, the emitted block
/// would accept the keywords the source rejects.
fn block_extra_params(params_node: Option<Node<'_>>, file: &str) -> IngestResult<Vec<crate::dialect::Param>> {
    use crate::dialect::Param;
    let Some(pn) = params_node
        .and_then(|node| node.as_block_parameters_node())
        .and_then(|node| node.parameters())
    else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for opt in pn.optionals().iter() {
        if let Some(op) = opt.as_optional_parameter_node() {
            let mut param = Param::positional(Symbol::from(constant_id_str(&op.name())));
            param.default = Some(ingest_expr(&op.value(), file)?);
            out.push(param);
        }
    }
    for kw in pn.keywords().iter() {
        if let Some(req) = kw.as_required_keyword_parameter_node() {
            let mut param = Param::positional(Symbol::from(constant_id_str(&req.name())));
            param.keyword = true;
            out.push(param);
        } else if let Some(opt) = kw.as_optional_keyword_parameter_node() {
            let mut param = Param::positional(Symbol::from(constant_id_str(&opt.name())));
            param.keyword = true;
            param.default = Some(ingest_expr(&opt.value(), file)?);
            out.push(param);
        }
    }
    if pn.keyword_rest().is_some_and(|node| node.as_no_keywords_parameter_node().is_some()) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "a block or lambda with `**nil` (no keywords accepted) has no representation".into(),
        });
    }
    if let Some(krp) = pn.keyword_rest().and_then(|node| node.as_keyword_rest_parameter_node()) {
        let name = krp.name().map(|loc| constant_id_str(&loc).to_string()).unwrap_or_default();
        let mut param = Param::positional(Symbol::from(name.as_str()));
        param.keyword = true;
        param.rest = true;
        out.push(param);
    }
    Ok(out)
}

fn block_param_names(params_node: Option<Node<'_>>) -> Vec<Symbol> {
    let Some(params_node) = params_node else { return vec![] };
    if params_node.as_it_parameters_node().is_some() {
        return vec![Symbol::from("it")];
    }
    let Some(bpn) = params_node.as_block_parameters_node() else {
        return vec![];
    };
    let Some(pn) = bpn.parameters() else { return vec![] };
    pn.requireds()
        .iter()
        .filter_map(|req| {
            if let Some(rp) = req.as_required_parameter_node() {
                return Some(Symbol::from(constant_id_str(&rp.name())));
            }
            // `|(host, secure), index|` — the destructured slot is a
            // temp the body unpacks first (`destructure_block_params`).
            req.as_multi_target_node().map(|_| nested_temp(&req, Span::synthetic()))
        })
        .collect()
}

/// The statements that unpack a block's destructured parameters
/// (`|(host, secure, origin), index|`) from the temps
/// `block_param_names` named for them, ahead of the body. Dropping the
/// slot instead left the body reading `host` / `secure` with nothing
/// bound — a NameError far from its cause.
fn destructure_block_params(params_node: Option<Node<'_>>, body: Expr, file: &str) -> IngestResult<Expr> {
    let Some(pn) = params_node
        .as_ref()
        .and_then(|n| n.as_block_parameters_node())
        .and_then(|b| b.parameters())
    else {
        return Ok(body);
    };
    let mut prelude: Vec<Expr> = Vec::new();
    for req in pn.requireds().iter() {
        let Some(mt) = req.as_multi_target_node() else { continue };
        let loc = req.location();
        let span = Span {
            file: super::sources::file_id(file),
            start: loc.start_offset() as u32,
            end: loc.end_offset() as u32,
        };
        let tmp = nested_temp(&req, span);
        let read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: tmp });
        prelude.extend(destructure_target(&mt, read, span, file)?);
    }
    if prelude.is_empty() {
        return Ok(body);
    }
    match *body.node {
        ExprNode::Seq { exprs } => prelude.extend(exprs),
        other => prelude.push(Expr { node: Box::new(other), ..body }),
    }
    Ok(Expr::new(Span::synthetic(), ExprNode::Seq { exprs: prelude }))
}

/// The block's REST parameter (`|*args|`), without its sigil.
///
/// Dropping it is not a degradation but a CORRUPTION: the body still
/// reads the name, and in an emitted module a bare `args` resolves to
/// whatever else answers that name — a module function, or nothing at
/// all. campfire's Opengraph tests stub a socket with
/// `.with { |*args, **| args.first == … }`, and unparameterized that
/// block died on `undefined local variable or method 'args'` from
/// inside the mocha matcher, naming nothing about the block.
///
/// An ANONYMOUS rest (`|*|`) has no name to bind and no body reference
/// to serve, so it stays absent. The trailing `**` in that same
/// campfire block is likewise nameless.
fn block_rest_param(params_node: Option<Node<'_>>) -> Option<Symbol> {
    let params_node = params_node?;
    let bpn = params_node.as_block_parameters_node()?;
    let pn = bpn.parameters()?;
    let rest = pn.rest()?;
    let rp = rest.as_rest_parameter_node()?;
    Some(Symbol::from(constant_id_str(&rp.name()?)))
}

/// The parameters AFTER a block's or lambda's rest (`|*, payload|`,
/// `->(*rest, a, b)`): Prism's `posts`, required names only.
fn block_post_params(params_node: Option<Node<'_>>) -> Vec<Symbol> {
    let Some(params_node) = params_node else { return vec![] };
    let Some(bpn) = params_node.as_block_parameters_node() else { return vec![] };
    let Some(pn) = bpn.parameters() else { return vec![] };
    pn.posts()
        .iter()
        .filter_map(|post| post.as_required_parameter_node())
        .map(|rp| Symbol::from(constant_id_str(&rp.name())))
        .collect()
}

/// Lambda IR has no slot for parameters after a rest, and dropping them
/// left the body reading names nothing bound: campfire's query counters
/// (`->(*, payload) { payload[:sql] }`) emitted as `-> { payload[:sql] }`.
/// Ruby's rule is that the rest takes everything but the trailing
/// parameters, so name the rest (`__rest` when it is anonymous) and pop
/// them off it, last first, ahead of the body. A named rest is left
/// holding exactly what Ruby gives it. The one difference: too few
/// arguments bind nil where a lambda would raise ArgumentError.
fn desugar_post_params(rest_param: &mut Option<Symbol>, posts: Vec<Symbol>, body: Expr) -> Expr {
    if posts.is_empty() {
        return body;
    }
    let rest = rest_param.get_or_insert_with(|| Symbol::from("__rest")).clone();
    let mut exprs: Vec<Expr> = posts
        .into_iter()
        .rev()
        .map(|name| {
            let pop = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: crate::ident::VarId(0), name: rest.clone() },
                    )),
                    method: Symbol::from("pop"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                    value: pop,
                },
            )
        })
        .collect();
    match *body.node {
        ExprNode::Seq { exprs: inner } => exprs.extend(inner),
        other => exprs.push(Expr::new(body.span, other)),
    }
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
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

fn nil_expr() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

/// Does this expression render with a leading `{`? True for a braced
/// hash literal and for anything whose receiver chain bottoms out in
/// one (`{a: 1}.merge(rest)`). Bare keyword args (`kwargs: true`)
/// render as `k: v`, so they're safe unparenthesized.
fn starts_with_brace_literal(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Hash { kwargs, .. } => !*kwargs,
        ExprNode::KeywordSplat { value } => starts_with_brace_literal(value),
        ExprNode::Send { recv: Some(recv), .. } => starts_with_brace_literal(recv),
        _ => false,
    }
}

/// Desugar a scrutinee-less `case` (`case / when <cond> / … / end`)
/// into the `if / elsif / else` chain it is shorthand for. Folds the
/// `when` clauses back-to-front so the first one ends up outermost;
/// a multi-condition `when a, b` ORs its conditions, as Ruby does.
fn ingest_condition_ladder(
    case: &ruby_prism::CaseNode<'_>,
    span: Span,
    file: &str,
) -> IngestResult<Expr> {
    // Innermost fallback: the `else` body, or `nil` — a scrutinee-less
    // `case` with no matching `when` and no `else` evaluates to nil.
    let mut chain = match case.else_clause().and_then(|e| e.statements()) {
        Some(s) => ingest_expr(&s.as_node(), file)?,
        None => nil_expr(),
    };
    let clauses: Vec<_> = case.conditions().iter().collect();
    for clause in clauses.iter().rev() {
        let when = clause.as_when_node().ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported case condition (expected when): {clause:?}"),
        })?;
        let body = match when.statements() {
            Some(s) => ingest_expr(&s.as_node(), file)?,
            None => nil_expr(),
        };
        let mut test: Option<Expr> = None;
        for c in when.conditions().iter() {
            let e = ingest_expr(&c, file)?;
            test = Some(match test {
                None => e,
                Some(left) => Expr::new(
                    Span::synthetic(),
                    ExprNode::BoolOp {
                        op: BoolOpKind::Or,
                        surface: BoolOpSurface::Symbol,
                        left,
                        right: e,
                    },
                ),
            });
        }
        chain = Expr::new(
            span,
            ExprNode::If {
                cond: test.unwrap_or_else(nil_expr),
                then_branch: body,
                else_branch: chain,
            },
        );
    }
    Ok(chain)
}

/// Ingest a hash literal (`{ … }`) or a bare keyword-argument list
/// (`foo(a: 1)`) into one Expr.
///
/// The common all-`key => value` case yields `ExprNode::Hash` exactly
/// as before. A double splat (`{ a: 1, **rest }`, `link_to url,
/// **attributes, data: …`) has no slot in `Hash`'s `Vec<(Expr, Expr)>`,
/// so it desugars into the `merge` chain it is defined to be —
/// left-to-right, later keys winning. That keeps the double splat out
/// of the IR entirely: no new `ExprNode` variant, no match arm in any
/// of the thirteen emitters.
///
/// A merge chain is a `Send`, not a `Hash`, so it loses the `kwargs`
/// flag and renders as a positional hash argument. That matches the
/// receiving end: `ingest_method_def` already models a `**rest`
/// parameter as a trailing *positional* param.
///
/// It does NOT match a callee declaring explicit keywords, which needs
/// the `**` to distribute the hash across them — erasing it hands the
/// callee one positional argument and it raises. Whether the erasure is
/// safe is thus a property of the CALLEE, which this purely local
/// desugar never sees; [`crate::lower::kwsplat`] runs post-analyze,
/// where the signature is resolvable, and puts the splat back as the
/// keywords it stood for.
fn ingest_hash_literal(
    elements: &ruby_prism::NodeList<'_>,
    kwargs: bool,
    span: Span,
    file: &str,
) -> IngestResult<Expr> {
    // Split into runs: consecutive `key => value` pairs collapse into one
    // Hash literal, each `**expr` becomes its own chain link.
    let mut chain: Option<Expr> = None;
    let mut pending: Vec<(Expr, Expr)> = Vec::new();
    let mut saw_splat = false;

    for el in elements.iter() {
        if let Some(assoc) = el.as_assoc_node() {
            let k = ingest_expr(&assoc.key(), file)?;
            let v = ingest_expr(&assoc.value(), file)?;
            pending.push((k, v));
            continue;
        }
        let Some(splat) = el.as_assoc_splat_node() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported hash element: {el:?}"),
            });
        };
        // Nameless call forwarding is an opaque packet, handled by the
        // argument-list walk. It cannot be used as an ordinary hash value.
        let value = splat.value().ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: "anonymous `**` outside a call argument is not supported".into(),
        })?;
        let value = ingest_expr(&value, file)?;
        saw_splat = true;
        chain = Some(merge_into(chain, std::mem::take(&mut pending), span, value));
    }

    if !saw_splat {
        return Ok(Expr::new(span, ExprNode::Hash { entries: pending, kwargs }));
    }
    let mut chain = chain.expect("saw_splat implies at least one link");
    if !pending.is_empty() {
        chain = merge_call(chain, Expr::new(span, ExprNode::Hash { entries: pending, kwargs: false }), span);
    }
    Ok(chain)
}

/// Append one `**value` link to a merge chain, first folding in any
/// literal pairs that preceded it.
fn merge_into(chain: Option<Expr>, pending: Vec<(Expr, Expr)>, span: Span, value: Expr) -> Expr {
    let base = match (chain, pending.is_empty()) {
        // Leading `**value` with nothing before it: the value *is* the
        // base. `{ **h }` copies in Ruby where this aliases; the copy
        // only matters if the callee mutates its options hash, which
        // no roundhouse runtime helper does.
        (None, true) => return value,
        (None, false) => Expr::new(span, ExprNode::Hash { entries: pending, kwargs: false }),
        (Some(chain), true) => return merge_call(chain, value, span),
        (Some(chain), false) => merge_call(
            chain,
            Expr::new(span, ExprNode::Hash { entries: pending, kwargs: false }),
            span,
        ),
    };
    merge_call(base, value, span)
}

fn merge_call(recv: Expr, arg: Expr, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("merge"),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    )
}

/// A receiver `&.` may read twice without changing what the program
/// does: a local, an ivar, a constant, `self`, or a literal.
fn is_plain_read(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::SelfRef
            | ExprNode::Lit { .. }
    )
}

/// A string literal whose bytes are not UTF-8: `"\x99*\n"` in a
/// serialized binary blob (a generated file's `descriptor_data`). A
/// `Literal::Str` holds UTF-8 text, and read lossily every invalid byte
/// became U+FFFD, so the emitted blob no longer parsed. The literal is the same bytes in the
/// same encoding instead, built from their hex digits:
/// `["0a21…"].pack("H*").force_encoding("UTF-8")`, then `.freeze` where
/// the file's `frozen_string_literal: true` froze the literal. Ruby gives
/// a `\x` escape outside ASCII in a UTF-8 source the UTF-8 encoding,
/// invalid (`"\xff".encoding == Encoding::UTF_8`), which Prism flags as
/// forced UTF-8; Prism's forced binary (a US-ASCII source) is
/// ASCII-8BIT, what `pack` returns. A literal in another source
/// encoding is refused by name.
fn binary_string(s: &ruby_prism::StringNode<'_>, span: Span, file: &str) -> IngestResult<ExprNode> {
    let encoding = if s.is_forced_binary_encoding() {
        None
    } else if s.is_forced_utf8_encoding() {
        Some("UTF-8")
    } else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "a non-UTF-8 string literal in a source encoding other than UTF-8 or US-ASCII is not modeled"
                .into(),
        });
    };
    let hex: String = s.unescaped().iter().map(|b| format!("{b:02x}")).collect();
    let lit = |value: String| Expr::new(span, ExprNode::Lit { value: Literal::Str { value } });
    let send = |recv: Expr, method: &str, args: Vec<Expr>| {
        Expr::new(
            span,
            ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: true },
        )
    };
    let array = Expr::new(span, ExprNode::Array { elements: vec![lit(hex)], style: crate::expr::ArrayStyle::Brackets });
    let mut value = send(array, "pack", vec![lit("H*".to_string())]);
    if let Some(encoding) = encoding {
        value = send(value, "force_encoding", vec![lit(encoding.to_string())]);
    }
    if s.is_frozen() {
        value = send(value, "freeze", Vec::new());
    }
    Ok(*value.node)
}
