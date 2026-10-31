//! A yielding call cannot return its receiver when its block always unwinds.
//! Track local proc aliases without confusing a lambda's local return with a
//! proc's return from the enclosing method.
use super::{BodyTyper, Ctx, Expr, ExprNode, LValue, Literal, Symbol, Ty};
use crate::det::HashMap;

#[derive(Clone, PartialEq)]
enum Exit {
    Return,
    LocalReturn,
    Uncertain,
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
            ExprNode::Lambda { body, .. } => {
                Some(self.exit_from(body, &mut Callbacks::default(), false))
            }
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
                Some(self.exit_from(body, &mut Callbacks::default(), method.as_str() == "proc"))
            }
            _ => None,
        }
    }

    fn exit_from(&self, expr: &Expr, env: &mut Callbacks, returns_escape: bool) -> Option<Exit> {
        let flow = |e: &Expr, env: &mut Callbacks| self.exit_from(e, env, returns_escape);
        match &*expr.node {
            // These reads and callback construction cannot rebind local aliases.
            ExprNode::Lambda { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::Lit { .. }
            | ExprNode::SelfRef => None,
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
            ExprNode::Break { .. } | ExprNode::Next { .. } => Some(Exit::LocalReturn),
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
                env.clear();
                if yes == no {
                    yes
                } else {
                    Some(Exit::Uncertain)
                }
            }
            ExprNode::BoolOp { left, .. } | ExprNode::While { cond: left, .. } => {
                let exit = flow(left, env);
                env.clear();
                exit
            }
            ExprNode::BeginRescue {
                body,
                rescues,
                ensure,
                ..
            } => {
                if let Some(exit) = ensure.as_ref().and_then(|e| flow(e, &mut env.clone())) {
                    return Some(exit);
                }
                let result = match flow(body, &mut env.clone()) {
                    Some(Exit::Raise(Some(class))) => {
                        let mut caught = None;
                        for rescue in rescues {
                            // An alias or an unmodeled subclass may catch this
                            // exception. A textual class name cannot prove a miss.
                            let mut catches = rescue.classes.is_empty();
                            for c in &rescue.classes {
                                let Some(Ty::Class { id, .. }) = c.ty.as_ref() else {
                                    env.clear();
                                    return None;
                                };
                                if !matches!(
                                    id.0.as_str(),
                                    "RuntimeError" | "TypeError" | "Exception" | "StandardError"
                                ) {
                                    env.clear();
                                    return None;
                                }
                                catches |= id.0 == class
                                    || matches!(id.0.as_str(), "Exception" | "StandardError");
                            }
                            if catches {
                                caught = Some(flow(&rescue.body, &mut env.clone()));
                                break;
                            }
                        }
                        caught.unwrap_or(Some(Exit::Raise(Some(class))))
                    }
                    Some(Exit::Raise(None)) if !rescues.is_empty() => None,
                    other => other,
                };
                env.clear();
                result
            }
            ExprNode::RescueModifier { expr, fallback } => {
                let result = match flow(expr, env) {
                    Some(Exit::Raise(_)) => flow(fallback, &mut env.clone()),
                    other => other,
                };
                env.clear();
                result
            }
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
                        env.clear();
                        return callback.filter(|e| !matches!(e, Exit::LocalReturn));
                    }
                    env.clear();
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
                if !self.kernel_callback_constructor(recv.as_ref(), method, args) {
                    env.clear();
                }
                None
            }
            _ => {
                env.clear();
                None
            }
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
            "Proc",
            "Exception",
            "StandardError",
            "RuntimeError",
            "TypeError",
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
                // load/create_additions and parse's object_class options can
                // construct a user object with its own non-yielding #tap.
                matches!(&*r.node, ExprNode::Send { recv, method, args, .. }
                if method.as_str() == "parse" && args.len() == 1
                    && self.json_value_source(recv.as_ref(), method, args))
            })
    }

    pub(super) fn block_has_pending_calls(&self, block: &Expr, ctx: &Ctx) -> bool {
        let ExprNode::Lambda { body, .. } = &*block.node else {
            return false;
        };
        let body = match &*body.node {
            ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
            _ => body,
        };
        let ExprNode::Send {
            recv,
            method,
            block: None,
            ..
        } = &*body.node
        else {
            return false;
        };
        if !matches!(body.ty, Some(Ty::Var { .. }))
            && !matches!(body.ty, Some(Ty::Untyped { why }) if why.is_pending_only())
        {
            return false;
        }
        // Only an unconditional call to a declared source helper is pending
        // control flow. Unknown proc calls and conditional calls are real
        // runtime uncertainty; treating them as pending would erase values.
        let owner = recv
            .as_ref()
            .and_then(|r| r.ty.as_ref())
            .or(ctx.self_ty.as_ref());
        let Some(Ty::Class { id, .. }) = owner else {
            return false;
        };
        let Some(class) = self.classes.get(id) else {
            return false;
        };
        let class_side = recv
            .as_ref()
            .map_or(ctx.class_side, |r| self.is_class_object(r, ctx));
        if class_side {
            class.source_class_methods.contains(method)
        } else {
            class.source_instance_methods.contains(method)
        }
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
