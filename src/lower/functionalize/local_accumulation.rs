//! Local mutable accumulation → functional rebinds (#29 pass #3, partial).
//!
//! Ruby builds collections/counters by mutating a local in place:
//!
//! ```text
//!   result = []                         result = []
//!   result.push("notice") if cond  ──▶  result = if cond, do: result ++ ["notice"], else: result
//!   result                              result
//! ```
//!
//! Elixir locals are immutable, so each in-place mutation becomes a
//! rebind of the same name:
//! - `x.push(v)`   → `x = x ++ [v]`
//! - `x[k] = v`    → `x = x.merge(%{k => v})`   (renders `Map.merge`)
//! - `x += n` (etc) → `x = x + n`               (via `desugar_op_assign`)
//!
//! Only targets/receivers that are plain locals (`ExprNode::Var`) are
//! rewritten. The conditional forms (`… if cond`) are left as the rebind
//! inside an `if`; the elixir2 walker's existing cond-rebind lift hoists
//! them to `x = if cond, do: <new>, else: x`. So this pass only does the
//! mutation→rebind step; the conditional handling is shared.

use crate::dialect::MethodDef;
use crate::expr::{desugar_op_assign, Expr, ExprNode, LValue};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

/// Rewrite local in-place accumulation in a method body into rebinds.
pub fn transform_method(mut m: MethodDef) -> MethodDef {
    m.body = rewrite(&m.body);
    m
}

fn rewrite(e: &Expr) -> Expr {
    match &*e.node {
        // `x += n` / `x -= n` / … on a local → `x = x <op> n`.
        ExprNode::OpAssign { target: target @ LValue::Var { .. }, op, value } => {
            desugar_op_assign(target, *op, &rewrite(value), Span::synthetic())
        }
        // `x.push(v)` on a local → `x = x ++ [v]`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "push" && args.len() == 1 =>
        {
            match local_name(r) {
                Some(name) => rebind(&name, binop(var(&name), "++", array(vec![rewrite(&args[0])]))),
                None => clone_send(e),
            }
        }
        // `x[k] = v` on a local → `x = x.merge(%{k => v})`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            match local_name(r) {
                Some(name) => {
                    let entry = (rewrite(&args[0]), rewrite(&args[1]));
                    rebind(&name, binop(var(&name), "merge", hash(vec![entry])))
                }
                None => clone_send(e),
            }
        }
        // Recurse through the containers these statements live in.
        ExprNode::Seq { exprs } => {
            syn(ExprNode::Seq { exprs: exprs.iter().map(rewrite).collect() })
        }
        ExprNode::If { cond, then_branch, else_branch } => syn(ExprNode::If {
            cond: rewrite(cond),
            then_branch: rewrite(then_branch),
            else_branch: rewrite(else_branch),
        }),
        ExprNode::Send { .. } => clone_send(e),
        ExprNode::Assign { target, value } => syn(ExprNode::Assign {
            target: target.clone(),
            value: rewrite(value),
        }),
        ExprNode::Return { value } => syn(ExprNode::Return { value: rewrite(value) }),
        _ => e.clone(),
    }
}

/// Recurse into a Send's receiver + args without otherwise changing it.
fn clone_send(e: &Expr) -> Expr {
    let ExprNode::Send { recv, method, args, block, parenthesized } = &*e.node else {
        return e.clone();
    };
    syn(ExprNode::Send {
        recv: recv.as_ref().map(rewrite),
        method: method.clone(),
        args: args.iter().map(rewrite).collect(),
        block: block.as_ref().map(rewrite),
        parenthesized: *parenthesized,
    })
}

fn local_name(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn rebind(name: &Symbol, value: Expr) -> Expr {
    syn(ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: name.clone() },
        value,
    })
}

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}
fn var(name: &Symbol) -> Expr {
    syn(ExprNode::Var { id: VarId(0), name: name.clone() })
}
fn binop(lhs: Expr, method: &str, rhs: Expr) -> Expr {
    syn(ExprNode::Send {
        recv: Some(lhs),
        method: Symbol::from(method),
        args: vec![rhs],
        block: None,
        parenthesized: false,
    })
}
fn array(elements: Vec<Expr>) -> Expr {
    syn(ExprNode::Array { elements, style: Default::default() })
}
fn hash(entries: Vec<(Expr, Expr)>) -> Expr {
    syn(ExprNode::Hash { entries, kwargs: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, LibraryClass, MethodReceiver, Param};
    use crate::effect::EffectSet;
    use crate::expr::{Literal, OpAssignOp};
    use crate::ident::ClassId;

    fn s(x: &str) -> Symbol {
        Symbol::from(x)
    }
    fn str_lit(x: &str) -> Expr {
        syn(ExprNode::Lit { value: Literal::Str { value: x.to_string() } })
    }
    fn send(recv: &str, method: &str, args: Vec<Expr>) -> Expr {
        syn(ExprNode::Send {
            recv: Some(var(&s(recv))),
            method: s(method),
            args,
            block: None,
            parenthesized: false,
        })
    }

    #[test]
    fn local_mutations_become_rebinds() {
        // def build(flag)
        //   result = []
        //   result.push("a") if flag
        //   result["k"] = "v"
        //   n = 0
        //   n += 1
        //   result
        // end
        let body = syn(ExprNode::Seq {
            exprs: vec![
                rebind(&s("result"), array(vec![])),
                syn(ExprNode::If {
                    cond: var(&s("flag")),
                    then_branch: send("result", "push", vec![str_lit("a")]),
                    else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
                }),
                send("result", "[]=", vec![str_lit("k"), str_lit("v")]),
                rebind(&s("n"), syn(ExprNode::Lit { value: Literal::Int { value: 0 } })),
                syn(ExprNode::OpAssign {
                    target: LValue::Var { id: VarId(0), name: s("n") },
                    op: OpAssignOp::Add,
                    value: syn(ExprNode::Lit { value: Literal::Int { value: 1 } }),
                }),
                var(&s("result")),
            ],
        });
        let m = MethodDef {
            name: s("build"),
            receiver: MethodReceiver::Class,
            params: vec![Param::positional(s("flag"))],
            block_param: None,
            body,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        };
        let out = transform_method(m);
        let class = LibraryClass {
            name: ClassId(s("Acc")),
            is_module: true,
            parent: None,
            includes: vec![],
            methods: vec![out],
            origin: None,
        };
        let ex = crate::emit::elixir2::emit_library_class(&class).expect("emit");
        eprintln!("--- accumulation ---\n{ex}\n--------------------");
        // push → list append, lifted through the conditional.
        assert!(ex.contains("result = if flag do"), "cond-rebind:\n{ex}");
        assert!(ex.contains("result ++ [\"a\"]"), "push → ++:\n{ex}");
        // []= → Map.merge.
        assert!(ex.contains("result = Map.merge(result, %{\"k\" => \"v\"})"), "[]= → merge:\n{ex}");
        // += → rebind.
        assert!(ex.contains("n = n + 1"), "OpAssign → rebind:\n{ex}");
        assert!(ex.trim_end().ends_with("result\n  end\nend"), "returns result:\n{ex}");
    }
}
