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
/// `publics` is this controller's OWN public actions; `inherited` names
/// the ones it reaches only through its parent. Both get a dispatch
/// arm, only the former gets a method.
pub(super) fn synthesize_process_action(
    preamble: &[PreambleStmt],
    publics: &[Action],
    inherited: &[Symbol],
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

    if !publics.is_empty() || !inherited.is_empty() {
        stmts.push(case_dispatch(publics, inherited));
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
/// `return if performed?` — Rails' halting semantics, as one statement.
/// Shared with `inline_before_filters`, which needs the same guard
/// after a filter body it prepended into an action.
pub(super) fn halt_if_performed() -> Expr {
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
    // Guard conjunction, in Rails' own order: the only/except action
    // check, then the `if:` / `unless:` conditions in BOTH spellings.
    //
    // The lambda spelling has been enforced since lobsters gated
    // dev-only filters with `if: -> { Rails.env.development? }`. The
    // SYMBOL spelling was carried and not enforced, which is a filter
    // that runs when Rails would have skipped it — campfire's
    // `before_action :reject_banned_ip, unless: :safe_request?` meant
    // every GET from a banned IP got a 429 where the app allows it, and
    // `block_banned_requests_test` said so in as many words: `expected
    // response :success, got status=429`.
    //
    // A symbol guard is a zero-arg predicate on the controller — the
    // same self-send the filter target itself is — so it builds the
    // same shape the lambda branch builds from a body expression. If
    // the predicate does not resolve, that is a real gap surfacing at
    // the call Rails also makes, not a new one.
    let mut conds: Vec<Expr> = Vec::new();
    if !(f.only.is_empty() && f.except.is_empty()) {
        conds.push(include_check(&f.only, &f.except));
    }
    let predicate = |name: &Symbol| {
        syn(ExprNode::Send {
            recv: None,
            method: name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    };
    let negate = |e: Expr| {
        syn(ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    };
    if let Some(name) = &f.if_cond {
        conds.push(predicate(name));
    }
    if let Some(name) = &f.unless_cond {
        conds.push(negate(predicate(name)));
    }
    if let Some(c) = &f.if_cond_expr {
        conds.push(c.clone());
    }
    if let Some(c) = &f.unless_cond_expr {
        conds.push(negate(c.clone()));
    }
    let Some(cond) = conds.into_iter().reduce(|l, r| {
        syn(ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: l,
            right: r,
        })
    }) else {
        return target_call;
    };
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
///
/// INHERITED actions get an arm too, and that is not a nicety. Rails
/// dispatches an action a controller reaches only through its parent —
/// campfire's `Rooms::DirectsController < RoomsController` defines
/// new/create/edit and inherits `destroy`, overriding just `room_scope`
/// and `ensure_can_administer` to widen who may run it. With no arm the
/// dispatcher fell off the end of the `case` and answered 200 with no
/// body, so `destroy only allowed for all room users` saw neither the
/// redirect nor the deleted row.
///
/// The preamble above already knew: `build_filter_preamble` emits
/// `set_room if [:show, :destroy].include?(action_name)` in that very
/// method. Two halves of one dispatcher disagreeing about which actions
/// exist is the shape of the bug.
///
/// An inherited arm needs no method here — Ruby finds the parent's, and
/// re-emitting it on the subclass would shadow an override rather than
/// use it.
fn case_dispatch(publics: &[Action], inherited: &[Symbol]) -> Expr {
    let arm_for = |action_name: &str, span: Option<crate::span::Span>| {
        let method_name = method_name_for_action(action_name);
        let mut dispatch = syn(ExprNode::Send {
            recv: None,
            method: Symbol::from(method_name),
            args: vec![],
            block: None,
            parenthesized: false,
        });
        // Each dispatch Send attributes to the action it invokes.
        if let Some(span) = span {
            dispatch.inherit_span(span);
        }
        Arm {
            pattern: Pattern::Lit {
                value: Literal::Sym { value: Symbol::from(action_name) },
            },
            guard: None,
            body: dispatch,
        }
    };
    let mut arms: Vec<Arm> =
        publics.iter().map(|a| arm_for(a.name.as_str(), Some(a.body.span))).collect();
    arms.extend(inherited.iter().map(|n| arm_for(n.as_str(), None)));
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
