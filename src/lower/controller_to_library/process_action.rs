//! Synthesize the `process_action(action_name)` dispatcher: conditionally
//! invoke before-action filters and case-dispatch to per-action methods.

use crate::dialect::{AccessorKind, Action, Filter, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Arm, ArrayStyle, Expr, ExprNode, Literal, Pattern};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

use super::util::method_name_for_action;

/// A statement in the synthesized before_action preamble — the filter
/// chain that runs ahead of the case dispatch. `Call` invokes a filter
/// method defined on this controller or an ancestor (`authenticate_user`
/// on ApplicationController firing for every subclass action); `Block`
/// inlines a block-form filter's body (`before_action { @page = page }`).
/// `halt_check` appends `return if performed?` after the statement —
/// Rails' halting semantics: a filter that renders or redirects skips
/// the action. It's set only when the filter body can respond, so
/// pure-assignment filters add no dispatch noise (and filter-free
/// controllers emit byte-identical dispatchers).
pub(super) enum PreambleStmt {
    Call { filter: Filter, halt_check: bool },
    Block { body: Expr, only: Vec<Symbol>, except: Vec<Symbol>, halt_check: bool },
}

/// Build the `process_action(action_name)` dispatcher:
///
/// ```ruby
/// def process_action(action_name)
///   authenticate_user
///   require_logged_in_user if [:hidden, :saved].include?(action_name)
///   return if performed?
///   case action_name
///   when :index then index
///   when :new then new_action
///   ...
///   end
/// end
/// ```
///
/// The preamble is the before_action chain (inherited filters first,
/// then this controller's own, declaration order); same-controller
/// filters whose targets are private methods are instead inlined into
/// the action bodies upstream (`inline_before_filters`) and don't
/// appear here.
pub(super) fn synthesize_process_action(
    preamble: &[PreambleStmt],
    publics: &[Action],
    enclosing_class: Symbol,
) -> MethodDef {
    let mut stmts: Vec<Expr> = Vec::new();

    for p in preamble {
        let (stmt, halt_check) = match p {
            PreambleStmt::Call { filter, halt_check } => {
                (filter_dispatch_stmt(filter), *halt_check)
            }
            PreambleStmt::Block { body, only, except, halt_check } => {
                let stmt = if only.is_empty() && except.is_empty() {
                    body.clone()
                } else {
                    syn(ExprNode::If {
                        cond: include_check(only, except),
                        then_branch: body.clone(),
                        else_branch: empty_seq(),
                    })
                };
                (stmt, *halt_check)
            }
        };
        stmts.push(stmt);
        if halt_check {
            stmts.push(halt_if_performed());
        }
    }

    if !publics.is_empty() {
        stmts.push(case_dispatch(publics));
    }

    let mut body = match stmts.len() {
        0 => syn(ExprNode::Seq { exprs: vec![] }),
        1 => stmts.into_iter().next().unwrap(),
        _ => syn(ExprNode::Seq { exprs: stmts }),
    };
    // Whole-cloth synthesis — attribute the dispatcher scaffolding to
    // the controller's source via its first public action (same file).
    // The per-arm dispatch Sends built in `case_dispatch` carry their
    // own action's span and win over this coarser stamp.
    if let Some(first) = publics.first() {
        body.inherit_span(first.body.span);
    }

    let action_name_param = Symbol::from("action_name");
    MethodDef {
        name: Symbol::from("process_action"),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(action_name_param.clone())],
        body,
        // process_action dispatches to the named action and returns
        // whatever it returns; concretely each action body terminates
        // in render/redirect (returns Nil), so dispatch returns Nil.
        signature: Some(crate::lower::typing::fn_sig(
            vec![(action_name_param, Ty::Sym)],
            Ty::Nil,
        )),
        effects: EffectSet::default(),
        enclosing_class: Some(enclosing_class),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// `return if performed?` — the halting check after a filter that can
/// render or redirect.
fn halt_if_performed() -> Expr {
    let performed = syn(ExprNode::Send {
        recv: None,
        method: Symbol::from("performed?"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    syn(ExprNode::If {
        cond: performed,
        then_branch: syn(ExprNode::Return {
            value: syn(ExprNode::Lit { value: Literal::Nil }),
        }),
        else_branch: empty_seq(),
    })
}

/// `set_X if [:a, :b, ...].include?(action_name)` — or unconditionally
/// (no filter `only:` / `except:`) just `set_X`.
fn filter_dispatch_stmt(f: &Filter) -> Expr {
    let target_call = syn(ExprNode::Send {
        recv: None,
        method: f.target.clone(),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    if f.only.is_empty() && f.except.is_empty() {
        return target_call;
    }
    let cond = include_check(&f.only, &f.except);
    syn(ExprNode::If {
        cond,
        then_branch: target_call,
        else_branch: empty_seq(),
    })
}

/// `[:a, :b].include?(action_name)` — or for `except:`,
/// `![:a, :b].include?(action_name)` (we pass the list through `not`
/// upstream; this helper just builds the include? form).
fn include_check(only: &[Symbol], except: &[Symbol]) -> Expr {
    let (syms, negate) = if !only.is_empty() {
        (only, false)
    } else {
        (except, true)
    };
    let array = syn(ExprNode::Array {
        elements: syms.iter().map(|s| sym_lit(s.as_str())).collect(),
        style: ArrayStyle::Brackets,
    });
    let include = syn(ExprNode::Send {
        recv: Some(array),
        method: Symbol::from("include?"),
        args: vec![var_ref("action_name")],
        block: None,
        parenthesized: true,
    });
    if negate {
        syn(ExprNode::Send {
            recv: Some(include),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    } else {
        include
    }
}

/// `case action_name; when :foo then foo; ...; end` — one arm per
/// public action. The `:new` action dispatches to `new_action` (Ruby
/// `def new` would shadow `Object#new`).
fn case_dispatch(publics: &[Action]) -> Expr {
    let arms: Vec<Arm> = publics
        .iter()
        .map(|a| {
            let action_name = a.name.as_str();
            let method_name = method_name_for_action(action_name);
            let mut dispatch = syn(ExprNode::Send {
                recv: None,
                method: Symbol::from(method_name),
                args: vec![],
                block: None,
                parenthesized: false,
            });
            // Each dispatch Send attributes to the action it invokes.
            dispatch.inherit_span(a.body.span);
            Arm {
                pattern: Pattern::Lit {
                    value: Literal::Sym { value: Symbol::from(action_name) },
                },
                guard: None,
                body: dispatch,
            }
        })
        .collect();
    syn(ExprNode::Case {
        scrutinee: var_ref("action_name"),
        arms,
    })
}

// ---------------------------------------------------------------------------
// Synthetic-Expr helpers — every node a synthesized span and default
// effects/ty so the rest of the pipeline doesn't choke on them.
// ---------------------------------------------------------------------------

fn syn(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn sym_lit(s: &str) -> Expr {
    syn(ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
}

fn var_ref(name: &str) -> Expr {
    syn(ExprNode::Var {
        id: VarId(0),
        name: Symbol::from(name),
    })
}

fn empty_seq() -> Expr {
    syn(ExprNode::Seq { exprs: vec![] })
}
