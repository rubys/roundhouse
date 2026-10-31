//! Prove the selector at the final framework-super boundary. The dispatcher
//! only implements its own action set; String conversion alone proves neither
//! that membership nor the Rails selector type contract.

use crate::det::{HashMap, HashSet};
use crate::dialect::{Action, Controller};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

type Names = Option<HashSet<Symbol>>;
type Bindings = HashMap<Symbol, Names>;

pub(in crate::lower::controller_to_library) fn check(
    controller: &Controller,
    all: &[Controller],
    dispatched: HashSet<Symbol>,
) {
    let mut chain = super::super::ancestor_chain(controller, all);
    chain.push(controller);
    let hooks: Vec<_> = chain
        .into_iter()
        .rev()
        .filter_map(|c| c.actions().find(|a| a.name.as_str() == "process_action"))
        .collect();
    // Request entry supplies one String selector from precisely this set.
    check_hook(&hooks, &[Some(dispatched.clone())], &dispatched, controller);
}

fn names(expr: &Expr, bindings: &Bindings) -> Names {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Some(std::iter::once(Symbol::from(value.as_str())).collect()),
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(std::iter::once(value.clone()).collect()),
        ExprNode::Var { name, .. } => bindings.get(name).cloned().flatten(),
        // Normalization is safe only after proving the input, including when
        // a source hook explicitly performs it before calling its parent.
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } if matches!(method.as_str(), "to_s" | "to_sym") && args.is_empty() => {
            names(recv, bindings)
        }
        _ => None,
    }
}

fn invalidate_writes(expr: &Expr, bindings: &mut Bindings) {
    let mut invalidate = |target: &LValue| {
        if let LValue::Var { name, .. } = target {
            bindings.remove(name);
        }
    };
    match &*expr.node {
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => invalidate(target),
        ExprNode::MultiAssign { targets, .. } => targets.iter().for_each(invalidate),
        ExprNode::Let { name, .. } => {
            bindings.remove(name);
        }
        ExprNode::Lambda {
            params,
            rest_param,
            extra_params,
            ..
        } => {
            for name in params
                .iter()
                .chain(rest_param.iter())
                .chain(extra_params.iter().map(|p| &p.name))
            {
                bindings.remove(name);
            }
        }
        ExprNode::BeginRescue { rescues, .. } => {
            for clause in rescues {
                if let Some(name) = &clause.binding {
                    bindings.remove(name);
                }
            }
        }
        ExprNode::CaseMatch { .. } => bindings.clear(),
        _ => {}
    }
    // Do not retain a finite selector proof after mutation or escape to an
    // arbitrary helper. Calls on derived values (name.class.to_s) are fine.
    if let ExprNode::Send {
        recv, method, args, ..
    } = &*expr.node
    {
        if let Some(recv) = recv
            && let ExprNode::Var { name, .. } = &*recv.node
            && !matches!(method.as_str(), "class" | "==" | "!=" | "to_s" | "to_sym")
        {
            bindings.remove(name);
        }
        for arg in args {
            if let ExprNode::Var { name, .. } = &*arg.node {
                bindings.remove(name);
            }
        }
    }
    if let ExprNode::Assign { value, .. } = &*expr.node
        && let ExprNode::Var { name, .. } = &*value.node
    {
        bindings.remove(name);
    }
    expr.node
        .for_each_child(&mut |child| invalidate_writes(child, bindings));
}

fn check_hook(
    hooks: &[&Action],
    arguments: &[Names],
    dispatched: &HashSet<Symbol>,
    receiver: &Controller,
) {
    let Some((hook, parents)) = hooks.split_first() else {
        return;
    };
    let params = hook.formal_params();
    let mut bindings = Bindings::default();
    for (i, param) in params.iter().enumerate() {
        let value = arguments.get(i).cloned().unwrap_or_else(|| {
            param
                .default
                .as_ref()
                .and_then(|expr| names(expr, &bindings))
        });
        bindings.insert(param.name.clone(), value);
    }
    invalidate_writes(&hook.body, &mut bindings);
    visit(&hook.body, &mut |expr| {
        let ExprNode::Super { args } = &*expr.node else {
            return;
        };
        let arguments: Vec<_> = match args {
            Some(args) => args.iter().map(|arg| names(arg, &bindings)).collect(),
            None => params
                .iter()
                .map(|p| bindings.get(&p.name).cloned().flatten())
                .collect(),
        };
        if !parents.is_empty() {
            // Ordinary source-parent calls retain their arguments and defaults.
            // Validate only what finally reaches the framework implementation.
            check_hook(parents, &arguments, dispatched, receiver);
            return;
        }
        let [selector] = arguments.as_slice() else {
            return;
        }; // Grounding diagnoses arity/forwarding.
        let message = match selector {
            Some(names) if names.iter().all(|name| dispatched.contains(name)) => return,
            Some(names) => {
                let mut missing: Vec<_> = names
                    .iter()
                    .filter(|name| !dispatched.contains(*name))
                    .map(Symbol::as_str)
                    .collect();
                missing.sort();
                format!(
                    "framework super selector {} is outside {}'s dispatched action set",
                    missing.join(", "),
                    receiver.name.0
                )
            }
            None => format!(
                "framework super selector must be a proven String or Symbol in {}'s dispatched action set; unknown or unsupported selector types cannot yet be dispatched",
                receiver.name.0
            ),
        };
        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            expr.span,
            None,
            "process_action_super",
            message,
        ));
    });
}

fn visit(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    expr.node.for_each_child(&mut |child| visit(child, f));
}
