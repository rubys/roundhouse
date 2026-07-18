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

/// The method name carried by a `:symbol` or `"string"` literal argument
/// (e.g. the first arg of `recv.try(:sym)`), if it is a literal.
fn literal_method_name(expr: &Expr) -> Option<String> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
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
            let parenthesized = c.opening_loc().is_some();
            let block = match c.block() {
                Some(block_node) => ingest_call_block(&block_node, file, &method)?,
                None => None,
            };
            // ActiveSupport `recv.try(:sym[, args])` — a nil-safe method
            // call. Lower to `recv && recv.sym(args)`, the same shape as
            // the `&.` desugar below. `try` is not core Ruby, and its
            // real definition is `respond_to?(name) && public_send(name,
            // …)` — dynamic dispatch AOT can't resolve — so the literal
            // method name is grounded here where it's statically known
            // (every corpus site passes a `:symbol`/`"string"` literal).
            // A dynamic method name is left as a plain `try` send.
            if method == "try" && block.is_none() && recv.is_some() {
                if let Some(name) = args.first().and_then(literal_method_name) {
                    let r = recv.unwrap();
                    let rest: Vec<Expr> = args.into_iter().skip(1).collect();
                    let call = Expr::new(
                        span,
                        ExprNode::Send {
                            recv: Some(r.clone()),
                            method: Symbol::from(name),
                            args: rest,
                            block: None,
                            parenthesized: true,
                        },
                    );
                    return Ok(Expr::new(
                        span,
                        ExprNode::BoolOp {
                            op: BoolOpKind::And,
                            surface: BoolOpSurface::Symbol,
                            left: r,
                            right: call,
                        },
                    ));
                }
            }
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
            let v: i32 = i.value().try_into().unwrap_or(0);
            ExprNode::Lit { value: Literal::Int { value: v as i64 } }
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
                kwargs: false,
            }
        }
        n if n.as_keyword_hash_node().is_some() => {
            // Bare keyword args `foo(a: 1)` arrive here when the arg list
            // is passed through generic expression ingest. No braces in source.
            let kh = n.as_keyword_hash_node().unwrap();
            ExprNode::Hash {
                entries: hash_entries_from(&kh.elements(), file)?,
                kwargs: true,
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
            let mw = n.as_multi_write_node().unwrap();
            // Only the simple `a, b = expr` shape — no splat (`*rest`)
            // and no post-rest targets. The fixture-driven scope.
            if mw.rest().is_some() {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "multi-write with splat (`a, *b = c`) not yet supported".into(),
                });
            }
            let rights: Vec<Node<'_>> = mw.rights().iter().collect();
            if !rights.is_empty() {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "multi-write with post-rest targets not yet supported".into(),
                });
            }
            let mut targets: Vec<crate::expr::LValue> = Vec::new();
            for left in mw.lefts().iter() {
                if let Some(lvt) = left.as_local_variable_target_node() {
                    targets.push(crate::expr::LValue::Var {
                        id: crate::ident::VarId(0),
                        name: Symbol::from(constant_id_str(&lvt.name())),
                    });
                } else if let Some(ivt) = left.as_instance_variable_target_node() {
                    let raw = constant_id_str(&ivt.name());
                    let name = raw.strip_prefix('@').unwrap_or(raw);
                    targets.push(crate::expr::LValue::Ivar { name: Symbol::from(name) });
                } else if let Some(it) = left.as_index_target_node() {
                    // `recv[index], … = …` — index write as a multi-write
                    // target (e.g. `link['href'], title, alt = attrs`).
                    let recv = ingest_expr(&it.receiver(), file)?;
                    let index = ingest_index_argument(it.arguments(), file)?;
                    targets.push(crate::expr::LValue::Index { recv, index });
                } else {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!("unsupported multi-write target: {left:?}"),
                    });
                }
            }
            let value = ingest_expr(&mw.value(), file)?;
            ExprNode::MultiAssign { targets, value }
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
                None => {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "case without scrutinee not yet supported".into(),
                    });
                }
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
                    ExprNode::Lambda {
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
