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


/// Extract one multi-write target — `a`, `@a`, or `recv[i]`
/// (e.g. `link['href'], title = attrs`) — as an `LValue`. Shared by the
/// leading targets and the trailing splat target of a `MultiAssign`.
fn multi_write_target(node: &Node<'_>, file: &str) -> IngestResult<crate::expr::LValue> {
    if let Some(lvt) = node.as_local_variable_target_node() {
        Ok(crate::expr::LValue::Var {
            id: crate::ident::VarId(0),
            name: Symbol::from(constant_id_str(&lvt.name())),
        })
    } else if let Some(ivt) = node.as_instance_variable_target_node() {
        let raw = constant_id_str(&ivt.name());
        let name = raw.strip_prefix('@').unwrap_or(raw);
        Ok(crate::expr::LValue::Ivar { name: Symbol::from(name) })
    } else if let Some(it) = node.as_index_target_node() {
        let recv = ingest_expr(&it.receiver(), file)?;
        let index = ingest_index_argument(it.arguments(), file)?;
        Ok(crate::expr::LValue::Index { recv, index })
    } else {
        Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported multi-write target: {node:?}"),
        })
    }
}

/// Ingest a `MultiWriteNode` (`a, b = …`, `a, *rest = …`). Split out of
/// the giant `ingest_expr_strict` match so its locals live in a frame
/// entered only for multi-writes, not on every deep recursive descent.
fn ingest_multi_write(
    mw: &ruby_prism::MultiWriteNode<'_>,
    span: Span,
    file: &str,
) -> IngestResult<ExprNode> {
    // Post-rest targets (`*init, last = c`) need length-relative
    // indexing off the tail; still out of scope.
    if mw.rights().iter().next().is_some() {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "multi-write with post-rest targets not yet supported".into(),
        });
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
    // `a, *rest = expr` — trailing splat. The shared IR has no rest-aware
    // destructuring node, so desugar to a temp bind + positional `[]`
    // reads + `rest = temp.drop(n)`. Every resulting node (Assign / `[]`
    // / `drop`) is already handled by analyze and all seven emitters, so
    // splat support stays contained to ingest rather than threading a
    // rest flag through ~15 MultiAssign consumers. The Seq ends in the
    // temp read so the whole expression's value is the RHS array —
    // matching Ruby, where `(a, *b = arr)` evaluates to `arr`.
    let tmp = Symbol::from(format!("__mw_{}", span.start).as_str());
    let tmp_read = || {
        Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: tmp.clone() })
    };
    let int_lit = |v: usize| {
        Expr::new(span, ExprNode::Lit { value: Literal::Int { value: v as i64 } })
    };
    let n_lefts = targets.len();
    let mut exprs: Vec<Expr> = Vec::with_capacity(n_lefts + 3);
    exprs.push(Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: tmp.clone() },
            value,
        },
    ));
    for (i, target) in targets.into_iter().enumerate() {
        let read = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(tmp_read()),
                method: Symbol::from("[]"),
                args: vec![int_lit(i)],
                block: None,
                parenthesized: true,
            },
        );
        exprs.push(Expr::new(span, ExprNode::Assign { target, value: read }));
    }
    // Anonymous splat (`a, * = c`) discards the rest — only a named
    // target gets a binding.
    if let Some(rest_node) = rest.as_splat_node().and_then(|s| s.expression()) {
        let rest_target = multi_write_target(&rest_node, file)?;
        let drop = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(tmp_read()),
                method: Symbol::from("drop"),
                args: vec![int_lit(n_lefts)],
                block: None,
                parenthesized: true,
            },
        );
        exprs.push(Expr::new(span, ExprNode::Assign { target: rest_target, value: drop }));
    }
    exprs.push(tmp_read());
    Ok(ExprNode::Seq { exprs })
}

fn ingest_expr_strict(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    // Byte offsets into the text registered for `file` (the exact text
    // prism is parsing). FileId(0) when the entry point didn't
    // register — spans then render message-only downstream.
    let loc = node.location();
    let span = Span {
        file: super::sources::file_id(file),
        start: loc.start_offset() as u32,
        end: loc.end_offset() as u32,
    };
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
            // sources verbatim (never through this path), so the idiom
            // survives where it matters.
            if method == "+@" && args.is_empty() && block.is_none() {
                if let Some(r) = &recv {
                    if matches!(
                        &*r.node,
                        ExprNode::Lit { value: Literal::Str { .. } }
                    ) {
                        return Ok(recv.unwrap());
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
            {
                let r = recv.unwrap();
                let mut defaults = args.into_iter().next().unwrap();
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
                && matches!(&*args[0].node, ExprNode::Hash { .. })
            {
                let r = recv.unwrap();
                let cond = args.into_iter().next().unwrap();
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
            // nil at runtime. Two documented divergences: the receiver
            // expression evaluates twice (harmless for the ivar/local
            // receivers real templates use), and a `false` receiver
            // skips the call where Ruby's `&.` would dispatch (nil is
            // the only value `&.` guards) — acceptable until a real
            // call site cares, at which point Send grows a `safe` flag.
            match (c.is_safe_navigation(), recv) {
                (true, Some(r)) => ExprNode::BoolOp {
                    op: crate::expr::BoolOpKind::And,
                    surface: crate::expr::BoolOpSurface::Symbol,
                    left: r,
                    right: Expr::new(span, send),
                },
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
        // Restrict to the bareword shape Prism produces for the
        // partial-local idiom: either a no-arg CallNode (when the name
        // isn't lexically bound, which is the partial-local case) or a
        // LocalVariableReadNode (when it IS bound). Both lift to a
        // `Var(name)` reference inside a marker Send. Other shapes
        // (`defined?(@ivar)`, `defined?(Foo)`, `defined?(obj.method)`)
        // have target-different semantics and surface as Unsupported
        // for now — lobsters/real-blog don't use them.
        //
        // The view-lowerer picks up the inner Var as a partial
        // parameter (collect_extra_params) then rewrites the marker
        // Send to `!name.nil?` (rewrite_defined_to_nil_check).
        n if n.as_defined_node().is_some() => {
            let d = n.as_defined_node().unwrap();
            let inner = d.value();
            // `defined?(@ivar)` — the memoization-guard idiom
            // (`return @x if defined?(@x)`), all over Mastodon's
            // ApplicationController. Lift the ivar read into the same
            // marker Send; the analyzer types `defined?` as `Str?` and
            // the ivar's type comes from its assignments, so the guard
            // costs nothing. (Class-body ivars aren't partial locals,
            // so the view-lowerer's Var-based rewrite never sees this
            // shape.)
            if let Some(iv) = inner.as_instance_variable_read_node() {
                let raw = constant_id_str(&iv.name());
                let name = raw.strip_prefix('@').unwrap_or(raw);
                let ivar = Expr::new(
                    Span::synthetic(),
                    ExprNode::Ivar { name: Symbol::from(name) },
                );
                return Ok(Expr::new(
                    span,
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("defined?"),
                        args: vec![ivar],
                        block: None,
                        parenthesized: true,
                    },
                ));
            }
            let name: Option<String> = if let Some(c) = inner.as_call_node() {
                let bareword = c.receiver().is_none()
                    && c.arguments().is_none()
                    && c.block().is_none();
                if bareword {
                    Some(constant_id_str(&c.name()).to_string())
                } else {
                    None
                }
            } else if let Some(lv) = inner.as_local_variable_read_node() {
                Some(constant_id_str(&lv.name()).to_string())
            } else {
                None
            };
            match name {
                Some(name) => {
                    let var = Expr::new(
                        Span::synthetic(),
                        ExprNode::Var {
                            id: crate::ident::VarId(0),
                            name: Symbol::from(name),
                        },
                    );
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("defined?"),
                        args: vec![var],
                        block: None,
                        parenthesized: true,
                    }
                }
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "`defined?` only supports bareword targets today: {inner:?}"
                        ),
                    });
                }
            }
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
            ExprNode::Lambda { rest_param: None, params, block_param: None, body, block_style }
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
        other => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported expression node: {other:?}"),
            });
        }
    };
    Ok(Expr::new(span, expr_node))
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
    // Raw parse, NOT the parse-diagnostic wrapper: `source` here is the
    // compiled-from-ERB buffer (or a seeds script), parsed out of its
    // true method-body context. Prism flags context-only errors on it —
    // a layout's `<%= yield %>` compiles to a top-level `yield`, which is
    // "Invalid yield" as a standalone program but legitimate in the view
    // method roundhouse ingests it into. Reporting those would be a false
    // positive on every layout, so this path stays silent (as it was
    // before the wrapper); real `.rb` source files report via the wrapper.
    let result = ruby_prism::parse(source.as_bytes());
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
fn ingest_call_block(
    node: &Node<'_>,
    file: &str,
    enclosing_method: &str,
) -> IngestResult<Option<Expr>> {
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
                    Expr::new(
                        Span::synthetic(),
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
                    ExprNode::Lambda { rest_param: None,
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
            // Other `&expr` shapes (`&method(:foo)`, `&proc { ... }`,
            // `&self.bar`) are not yet supported. Filing this as
            // unsupported keeps the error surface narrow — the local-
            // variable case covers the `&block` forwarding idiom that
            // motivates issue #25.
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "block-argument forms other than `&:symbol` and `&local_var` not yet supported".into(),
            });
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
    let params = block_param_names(&b);
    let rest_param = block_rest_param(&b);
    let body = match b.body() {
        Some(body) => ingest_expr(&body, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    let block_style = block_style_from_opening(b.opening_loc().as_slice());
    Ok(Some(Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { rest_param, params, block_param: None, body, block_style },
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
    if params_node.as_it_parameters_node().is_some() {
        return vec![Symbol::from("it")];
    }
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
fn block_rest_param(b: &ruby_prism::BlockNode<'_>) -> Option<Symbol> {
    let params_node = b.parameters()?;
    let bpn = params_node.as_block_parameters_node()?;
    let pn = bpn.parameters()?;
    let rest = pn.rest()?;
    let rp = rest.as_rest_parameter_node()?;
    Some(Symbol::from(constant_id_str(&rp.name()?)))
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
        // Anonymous `**` forwarding (`def f(**) ; g(**) ; end`) has no
        // value to merge, and the declaration side drops the unnamed
        // parameter — fail loud rather than emit a silently empty hash.
        let Some(value) = splat.value() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "anonymous `**` keyword forwarding not yet supported".into(),
            });
        };
        saw_splat = true;
        let value = ingest_expr(&value, file)?;
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
