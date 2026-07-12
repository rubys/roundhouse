//! Block-form record-factory inlining: Rails'
//! `X.create! do |kv| ... end` becomes the call-site sequence
//!
//!   kv = X.new(attrs)
//!   <block body>
//!   raise ActiveRecord::RecordInvalid, kv unless kv.save   # bang form
//!   kv
//!
//! The runtime's `create`/`create!` stay BLOCKLESS on purpose — a
//! `yield` there forces a block param onto all twelve transpiled
//! runtimes (breaking their 1-arg callers), and under spinel AOT the
//! inherited-factory yield types the block param as Base rather than
//! the calling subclass (matz/spinel#2158). Inlining sidesteps both:
//! `kv` is a plain local born from `X.new`, typed concretely
//! everywhere. Semantics match the runtime bodies (yield-before-save;
//! the bang form's raise-unless-save).
//!
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target consumes the inlined form. A `create`/`create!` whose
//! block isn't the single-param lambda shape stays put and joins the
//! residue ledger: the blockless runtime factory would silently ignore
//! the block (Ruby doesn't fault an unused block), which is a behavior
//! change worth a name, not a silence.

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// Inline block-form `create`/`create!` sends across every hook body.
/// Returns the residue ledger: block-carrying factory calls left in
/// source shape, with the reason.
pub fn apply_create_block_inline(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| rewrite_create_block(body, &mut diags));
    diags
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("create_block_inline"),
        construct: Symbol::from("create-with-block"),
        reason: Symbol::from(reason),
    };
    Diagnostic {
        span: expr.span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "block-form `create` left uninlined ({reason}) — the runtime \
             factory is blockless and would silently ignore the block"
        ),
    }
}

fn rewrite_create_block(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_create_block(c, diags));
    let recognized = matches!(
        &*expr.node,
        ExprNode::Send { method, block: Some(_), .. }
            if matches!(method.as_str(), "create" | "create!")
    );
    if !recognized {
        return;
    }
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { block: Some(b), .. }
            if matches!(&*b.node, ExprNode::Lambda { params, .. } if params.len() == 1)
    );
    if !matches {
        diags.push(residue(expr, "block is not a single-param lambda"));
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, method, args, block: Some(block), .. } = node else {
        unreachable!()
    };
    let ExprNode::Lambda { params, body, .. } = *block.node else { unreachable!() };
    let name = params.into_iter().next().expect("single param checked");
    // Reuse the block body's own VarId for the binding so the body's
    // reads reference the local we assign.
    let var_id = find_var_id(&body, &name).unwrap_or(crate::ident::VarId(0));
    let var = |sp| Expr::new(sp, ExprNode::Var { id: var_id, name: name.clone() });

    // In a class-method body `self.new` and bare `new` are identical
    // Ruby; emit the bare form (spinel's explicit-self dispatch fix
    // doesn't cover the builtin `new` yet — noted on matz/spinel#2157).
    let new_recv = match &recv {
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => None,
        other => other.clone(),
    };
    let assign = Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { id: var_id, name: name.clone() },
            value: Expr::new(
                span,
                ExprNode::Send {
                    recv: new_recv,
                    method: Symbol::from("new"),
                    args,
                    block: None,
                    parenthesized: true,
                },
            ),
        },
    );
    let save = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(var(span)),
            method: Symbol::from("save"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let save_step = if method.as_str() == "create!" {
        // raise ActiveRecord::RecordInvalid, kv unless kv.save
        Expr::new(
            span,
            ExprNode::If {
                cond: save,
                then_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                else_branch: Expr::new(
                    span,
                    ExprNode::Send {
                        recv: None,
                        method: Symbol::from("raise"),
                        args: vec![
                            Expr::new(
                                span,
                                ExprNode::Const {
                                    path: vec![
                                        Symbol::from("ActiveRecord"),
                                        Symbol::from("RecordInvalid"),
                                    ],
                                },
                            ),
                            var(span),
                        ],
                        block: None,
                        parenthesized: false,
                    },
                ),
            },
        )
    } else {
        save
    };
    *expr.node = ExprNode::Seq { exprs: vec![assign, body, save_step, var(span)] };
    expr.ty = None;
}

fn find_var_id(e: &Expr, name: &Symbol) -> Option<crate::ident::VarId> {
    if let ExprNode::Var { id, name: n } = &*e.node {
        if n == name {
            return Some(*id);
        }
    }
    let mut found = None;
    e.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = find_var_id(c, name);
        }
    });
    found
}
