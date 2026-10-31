//! A yielding call cannot return its receiver when its block always unwinds.
//! Track local proc aliases without confusing a lambda's local return with a
//! proc's return from the enclosing method.
use super::{BodyTyper, Expr, ExprNode, LValue, Literal, Symbol, Ty};
use crate::det::HashMap;

#[derive(Clone, PartialEq)]
enum Exit {
    Return,
    LocalReturn,
    Raise(Option<Symbol>),
}

type Callbacks = HashMap<Symbol, Option<Exit>>;

impl BodyTyper<'_> {
    pub(super) fn kernel_callback_constructor(
        &self,
        recv: Option<&Expr>,
        method: &Symbol,
        args: &[Expr],
    ) -> bool {
        args.is_empty()
            && matches!(method.as_str(), "proc" | "lambda")
            && self.kernel_receiver(recv)
    }

    fn kernel_receiver(&self, recv: Option<&Expr>) -> bool {
        recv.is_some_and(|r| {
            matches!(&*r.node, ExprNode::Const { path }
            if path.iter().map(Symbol::as_str).filter(|p| !p.is_empty()).eq(["Kernel"]))
                && matches!(r.ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "Kernel")
        }) && self
            .const_resolver
            .as_ref()
            .is_some_and(|r| !r.has_source_namespace("Kernel"))
    }

    fn callback_exit(&self, value: &Expr, env: &Callbacks) -> Option<Option<Exit>> {
        match &*value.node {
            ExprNode::Var { name, .. } => env.get(name).cloned(),
            ExprNode::Lambda { body, .. } => Some(self.exit_from(body, &mut env.clone(), false)),
            ExprNode::Send {
                recv,
                method,
                args,
                block: Some(block),
                ..
            } if self.kernel_receiver(recv.as_ref())
                && args.is_empty()
                && matches!(method.as_str(), "proc" | "lambda") =>
            {
                let ExprNode::Lambda { body, .. } = &*block.node else {
                    return None;
                };
                Some(self.exit_from(body, &mut env.clone(), method.as_str() == "proc"))
            }
            _ => None,
        }
    }

    fn exit_from(&self, expr: &Expr, env: &mut Callbacks, returns_escape: bool) -> Option<Exit> {
        let flow = |e: &Expr, env: &mut Callbacks| self.exit_from(e, env, returns_escape);
        match &*expr.node {
            ExprNode::Lambda { .. } => None, // Creating a callback does not call it.
            ExprNode::Return { .. } => Some(if returns_escape {
                Exit::Return
            } else {
                Exit::LocalReturn
            }),
            ExprNode::Raise { value } => Some(Exit::Raise(match &*value.node {
                ExprNode::Lit {
                    value: Literal::Str { .. },
                } => Some(Symbol::from("RuntimeError")),
                _ => None,
            })),
            ExprNode::Break { .. } | ExprNode::Next { .. } => None,
            ExprNode::Seq { exprs } => {
                for e in exprs {
                    if let Some(exit) = flow(e, env) {
                        return Some(exit);
                    }
                }
                None
            }
            ExprNode::Assign {
                target: LValue::Var { name, .. },
                value,
            } => {
                if let Some(exit) = flow(value, env) {
                    return Some(exit);
                }
                if let Some(callback) = self.callback_exit(value, env) {
                    env.insert(name.clone(), callback);
                } else {
                    env.remove(name);
                }
                None
            }
            ExprNode::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if let Some(exit) = flow(cond, env) {
                    return Some(exit);
                }
                let yes = flow(then_branch, &mut env.clone());
                let no = flow(else_branch, &mut env.clone());
                if yes == no { yes } else { None }
            }
            ExprNode::BoolOp { left, .. } => flow(left, env),
            ExprNode::While { cond, .. } => flow(cond, env),
            ExprNode::BeginRescue {
                body,
                rescues,
                ensure,
                ..
            } => {
                if let Some(exit) = ensure.as_ref().and_then(|e| flow(e, &mut env.clone())) {
                    return Some(exit);
                }
                match flow(body, &mut env.clone()) {
                    Some(Exit::Raise(Some(class))) => {
                        for rescue in rescues {
                            let catches = rescue.classes.is_empty() || rescue.classes.iter().any(|c| {
                                matches!(&*c.node, ExprNode::Const { path } if {
                                    let name = path.iter().map(Symbol::as_str).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                                    name == class.as_str() || matches!(name.as_str(), "Exception" | "StandardError")
                                })
                            });
                            if catches {
                                return flow(&rescue.body, &mut env.clone());
                            }
                        }
                        Some(Exit::Raise(Some(class)))
                    }
                    Some(Exit::Raise(None)) if !rescues.is_empty() => None,
                    other => other,
                }
            }
            ExprNode::RescueModifier { expr, fallback } => match flow(expr, env) {
                Some(Exit::Raise(_)) => flow(fallback, &mut env.clone()),
                other => other,
            },
            ExprNode::Send {
                recv,
                method,
                args,
                block,
                ..
            } => {
                if let Some(exit) = recv.as_ref().and_then(|r| flow(r, env)) {
                    return Some(exit);
                }
                for arg in args {
                    if let Some(exit) = flow(arg, env) {
                        return Some(exit);
                    }
                }
                if matches!(method.as_str(), "call" | "[]") {
                    if let Some(callback) = recv.as_ref().and_then(|r| self.callback_exit(r, env)) {
                        return callback.filter(|e| !matches!(e, Exit::LocalReturn));
                    }
                    return None;
                }
                if self.kernel_receiver(recv.as_ref())
                    && matches!(method.as_str(), "raise" | "fail")
                {
                    let class = match args.first().map(|a| &*a.node) {
                        Some(ExprNode::Lit {
                            value: Literal::Str { .. },
                        }) => Some(Symbol::from(if args.len() == 1 {
                            "RuntimeError"
                        } else {
                            "TypeError"
                        })),
                        Some(ExprNode::Const { path }) => Some(Symbol::from(
                            path.iter()
                                .map(Symbol::as_str)
                                .filter(|p| !p.is_empty())
                                .collect::<Vec<_>>()
                                .join("::"),
                        )),
                        _ => None,
                    };
                    return Some(Exit::Raise(class));
                }
                // An ordinary helper whose inferred return is never has no
                // normal value. A callback's Bottom is handled above: its
                // local return need not unwind this call.
                if block.is_none() && matches!(expr.ty, Some(Ty::Bottom)) {
                    return Some(Exit::Raise(None));
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn core_yielding_call(
        &self,
        recv: Option<&Expr>,
        ty: Option<&Ty>,
        method: &Symbol,
    ) -> bool {
        let Some(resolver) = self.const_resolver.as_ref() else {
            return false;
        };
        if [
            "BasicObject",
            "Object",
            "Kernel",
            "Array",
            "Hash",
            "String",
            "Integer",
            "Float",
            "TrueClass",
            "FalseClass",
            "NilClass",
        ]
        .iter()
        .any(|name| resolver.has_source_namespace(name))
        {
            return false;
        }
        ty.is_some_and(|t| self.answers_receiver(t, method))
            || recv.is_some_and(|r| {
                matches!(&*r.node, ExprNode::Send { recv, method, args, .. }
                if self.json_value_source(recv.as_ref(), method, args))
            })
    }

    pub(super) fn block_has_pending_calls(&self, block: &Expr) -> bool {
        fn pending(typer: &BodyTyper<'_>, expr: &Expr) -> bool {
            let scan = |e: &Expr| pending(typer, e);
            match &*expr.node {
                ExprNode::Lambda { .. } => false,
                ExprNode::Send {
                    recv,
                    args,
                    block,
                    method,
                    ..
                } => {
                    // Creating a core callback cannot execute its body, even
                    // while its callable signature has not been harvested.
                    if typer.kernel_callback_constructor(recv.as_ref(), method, args) {
                        return recv.as_ref().is_some_and(scan) || args.iter().any(scan);
                    }
                    if matches!(expr.ty, Some(Ty::Var { .. }))
                        || matches!(expr.ty, Some(Ty::Untyped { why }) if why.is_pending_only())
                    {
                        return true;
                    }
                    recv.as_ref().is_some_and(scan)
                        || args.iter().any(scan)
                        || (!matches!(method.as_str(), "proc" | "lambda" | "define_method")
                            && block.as_ref().is_some_and(|b| match &*b.node {
                                ExprNode::Lambda { body, .. } => scan(body),
                                _ => false,
                            }))
                }
                _ => {
                    let mut found = false;
                    expr.node.for_each_child(&mut |c| found |= scan(c));
                    found
                }
            }
        }
        let ExprNode::Lambda { body, .. } = &*block.node else {
            return false;
        };
        pending(self, body)
    }

    pub(super) fn block_always_unwinds(&self, block: &Expr) -> bool {
        let ExprNode::Lambda { body, .. } = &*block.node else {
            return false;
        };
        matches!(
            self.exit_from(body, &mut HashMap::default(), true),
            Some(Exit::Return | Exit::Raise(_))
        )
    }
}
