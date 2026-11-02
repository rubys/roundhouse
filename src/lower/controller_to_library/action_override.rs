//! The app's `process_action` sits above the framework callback/dispatch
//! implementation. Keep that lookup order even though the latter is
//! specialized into each concrete controller.

use crate::dialect::{Controller, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{Symbol, VarId};

pub(super) const DISPATCH: &str = "_roundhouse_process_action";
pub(crate) const ENTRY: &str = "_roundhouse_dispatch_action";

mod selectors;
pub(super) use selectors::check as check_selectors;

fn defines_override(controller: &Controller) -> bool {
    controller
        .actions()
        .any(|a| a.name.as_str() == "process_action")
}

pub(super) fn overrides(controller: &Controller, all: &[Controller]) -> bool {
    defines_override(controller)
        || super::ancestor_chain(controller, all)
            .iter()
            .any(|c| defines_override(c))
}

/// A direct call to an action does not run Rails callbacks. An ancestor's
/// action can be called directly by a descendant's override too.
pub(super) fn may_inline_callbacks(controller: &Controller, all: &[Controller]) -> bool {
    !overrides(controller, all)
        && !all.iter().any(|c| {
            defines_override(c)
                && super::ancestor_chain(c, all)
                    .iter()
                    .any(|p| p.name == controller.name)
        })
}

/// Concern expansion keeps the winning method, not the hidden definitions
/// reached by its super. Do not claim support for a chain that expansion has
/// already discarded; a single concern override has no such hidden link.
pub(super) fn check_source_chain(
    controller: &Controller,
    libraries: &[LibraryClass],
    all: &[Controller],
) {
    if !may_inline_callbacks(controller, all) {
        for item in &controller.body {
            if let crate::dialect::ControllerBodyItem::Unknown { expr, .. } = item {
                // An inherited alias may never materialize in the lowered
                // method list. Its destination still occupies this namespace.
                if let ExprNode::Send {
                    recv, method, args, ..
                } = &*expr.node
                    && recv
                        .as_ref()
                        .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && method.as_str() == "alias_method"
                    && let Some(name) = args.first().and_then(|arg| match &*arg.node {
                        ExprNode::Lit {
                            value: crate::expr::Literal::Sym { value },
                        } => Some(value.as_str()),
                        ExprNode::Lit {
                            value: crate::expr::Literal::Str { value },
                        } => Some(value.as_str()),
                        _ => None,
                    })
                    && matches!(name, DISPATCH | ENTRY)
                {
                    crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                        expr.span,
                        None,
                        "controller_dispatch_collision",
                        format!(
                            "{}#{name} is an application alias that conflicts with a specialized framework method",
                            controller.name.0
                        ),
                    ));
                }
                for name in crate::ingest::delegate::declared_names(expr) {
                    if matches!(name.as_str(), DISPATCH | ENTRY) {
                        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                            expr.span,
                            None,
                            "controller_dispatch_collision",
                            format!(
                                "{}#{name} is an application delegate that conflicts with a specialized framework method",
                                controller.name.0
                            ),
                        ));
                    }
                }
            }
        }
    }
    if super::alias_method_pairs(controller)
        .iter()
        .any(|(name, _)| name.as_str() == "process_action")
    {
        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            controller.parent_span,
            None,
            "process_action_alias",
            "an aliased process_action override cannot yet retain its super chain",
        ));
    }
    let Some(action) = controller
        .actions()
        .find(|a| a.name.as_str() == "process_action")
    else {
        return;
    };
    if !super::calls_super(&action.body) {
        return;
    }
    // The original module methods may already have been pruned after
    // splicing. Both controller copies still retain the aliased body's
    // span, even though the alias has a new name span.
    if controller
        .actions()
        .any(|other| other.name != action.name && other.body.span == action.body.span)
    {
        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            action.name_span,
            None,
            "process_action_alias",
            "an aliased concern hook cannot yet retain the original method's super name",
        ));
        return;
    }
    let mut pending: Vec<_> = crate::analyze::controller_include_statements(controller)
        .into_iter()
        .flat_map(|(_, group)| group)
        .collect();
    let mut seen = crate::det::HashSet::default();
    let mut hidden = false;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        for module in libraries.iter().filter(|lc| lc.name == id) {
            // An alias keeps the original body's span and super
            // name. Expansion copied its body but not that name.
            if module.methods.iter().any(|m| {
                m.receiver == MethodReceiver::Instance
                    && m.body.span == action.body.span
                    && m.name.as_str() != "process_action"
            }) {
                crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                    action.body.span,
                    None,
                    "process_action_alias",
                    "an aliased concern hook cannot yet retain the original method's super name",
                ));
                return;
            }
            pending.extend(module.includes.iter().cloned());
            hidden |= module.methods.iter().any(|m| {
                m.receiver == MethodReceiver::Instance
                    && m.name.as_str() == "process_action"
                    && m.name_span != action.name_span
            });
        }
    }
    if hidden {
        crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
            action.name_span,
            None,
            "process_action_concern_super",
            "super reaches a hidden concern override whose method chain was not retained",
        ));
    }
}

/// Inspect generated accessors, aliases and delegates too. A specialization
/// on a child must not hide an app method of the internal name on a parent.
pub(super) fn check_ancestor_collisions(classes: &[LibraryClass], controllers: &[Controller]) {
    for controller in controllers.iter().filter(|c| overrides(c, controllers)) {
        for parent in
            std::iter::once(controller).chain(super::ancestor_chain(controller, controllers))
        {
            let collision = classes
                .iter()
                .filter(|lc| lc.name == parent.name)
                .flat_map(|lc| &lc.methods)
                .find(|m| {
                    m.receiver == MethodReceiver::Instance
                        && matches!(m.name.as_str(), DISPATCH | ENTRY)
                });
            if let Some(method) = collision {
                crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                    method.name_span,
                    None,
                    "controller_dispatch_collision",
                    format!(
                        "{}#{} conflicts with the specialized framework dispatcher or request adapter",
                        parent.name.0, method.name
                    ),
                ));
            }
        }
    }
}

pub(super) fn finish(methods: &mut [MethodDef], controller: &Controller, all: &[Controller]) {
    if !overrides(controller, all) {
        return;
    }
    // A framework specialization must not shadow an app-defined method,
    // including one inherited from an otherwise action-less base.
    for c in std::iter::once(controller).chain(super::ancestor_chain(controller, all)) {
        if let Some(action) = c
            .actions()
            .find(|a| matches!(a.name.as_str(), DISPATCH | ENTRY))
        {
            crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                action.name_span,
                None,
                "controller_dispatch_collision",
                format!(
                    "{}#{} conflicts with the specialized framework dispatcher or request adapter",
                    c.name.0, action.name
                ),
            ));
        }
    }
    // Intermediate controllers have no generated process_action when an
    // ancestor owns the override. Ordinary super therefore reaches that
    // source method. Only the last app override needs grounding: its super
    // reaches the framework, specialized for the actual receiving controller.
    if super::ancestor_chain(controller, all)
        .iter()
        .any(|c| defines_override(c))
    {
        return;
    }
    for method in methods {
        if method.receiver != MethodReceiver::Instance || method.name.as_str() != "process_action" {
            continue;
        }
        let implicit = method
            .params
            .iter()
            .map(|p| {
                Expr::new(
                    method.name_span,
                    ExprNode::Var {
                        id: VarId(0),
                        name: p.name.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let simple = method.block_param.is_none()
            && !method.has_anonymous_block
            && method.params.iter().all(|p| {
                !p.keyword && !p.rest && !p.forwarding && !p.from_keyword && !p.from_kwrest
            });
        ground_super(&mut method.body, &implicit, simple);
    }
}

fn ground_super(body: &mut Expr, implicit: &[Expr], simple: bool) {
    if let ExprNode::Super { args } = &*body.node {
        let args = match args {
            Some(args) => args.clone(),
            None if simple => implicit.to_vec(),
            None => {
                crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                    body.span,
                    None,
                    "process_action_super",
                    "implicit super with rest, keyword or block parameters cannot yet be grounded",
                ));
                return;
            }
        };
        if args.len() != 1
            || args.iter().any(|arg| {
                matches!(
                    &*arg.node,
                    ExprNode::Splat { .. }
                        | ExprNode::KeywordSplat { .. }
                        | ExprNode::ForwardArgs
                        | ExprNode::ForwardKeywords
                        | ExprNode::ForwardKeywordsWithPairs { .. }
                        | ExprNode::Hash { .. }
                )
            })
        {
            crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                body.span,
                None,
                "process_action_super",
                "framework super requires exactly one action selector; zero, extra, splatted, keyword or forwarded arguments cannot yet be dispatched",
            ));
            return;
        }
        let selector = Expr::new(
            body.span,
            ExprNode::Send {
                recv: Some(args.into_iter().next().unwrap()),
                method: Symbol::from("to_s"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        );
        *body.node = ExprNode::Send {
            recv: Some(Expr::new(body.span, ExprNode::SelfRef)),
            method: Symbol::from(DISPATCH),
            args: vec![selector],
            block: None,
            parenthesized: true,
        };
    }
    body.node
        .for_each_child_mut(&mut |child| ground_super(child, implicit, simple));
}

/// Response statements also have Ruby values: render answers the body,
/// head answers true, and redirect_to answers the assigned status. Retain
/// those at source call sites; generated implicit rendering stays a tail.
pub(super) fn retain_response_values(body: &Expr) -> Expr {
    use crate::expr::Literal;
    super::util::map_expr(body, &|expr| {
        let ExprNode::Send { recv, method, .. } = &*expr.node else {
            return None;
        };
        if recv
            .as_ref()
            .is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef))
        {
            return None;
        }
        let value = match method.as_str() {
            // Response storage is backend-specific (Ruby isolates it from
            // application ivars). Read the framework accessor, not @body.
            "render" => ExprNode::Send {
                recv: Some(Expr::new(expr.span, ExprNode::SelfRef)),
                method: Symbol::from("body"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
            "head" => ExprNode::Lit {
                value: Literal::Bool { value: true },
            },
            "redirect_to" => ExprNode::Send {
                recv: Some(Expr::new(expr.span, ExprNode::SelfRef)),
                method: Symbol::from("status"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
            _ => return None,
        };
        Some(Expr::new(
            expr.span,
            ExprNode::Seq {
                exprs: vec![expr.clone(), Expr::new(expr.span, value)],
            },
        ))
    })
}

pub(super) fn request_adapter(controller: &Controller, _reads_action_name: bool) -> MethodDef {
    use crate::dialect::{AccessorKind, MethodVisibility, Param};
    use crate::span::Span;
    use crate::ty::Ty;
    let span = Span::synthetic();
    let arg = || {
        Expr::new(
            span,
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from("selector"),
            },
        )
    };
    let mut exprs = Vec::new();
    {
        exprs.push(Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)),
                method: Symbol::from("assign_action_name"),
                args: vec![arg()],
                block: None,
                parenthesized: true,
            },
        ));
    }
    exprs.push(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from("process_action"),
            args: vec![Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(arg()),
                    method: Symbol::from("to_s"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            )],
            block: None,
            parenthesized: true,
        },
    ));
    MethodDef {
        name: Symbol::from(ENTRY),
        name_span: span,
        visibility: MethodVisibility::Public,
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(Symbol::from("selector"))],
        body: Expr::new(span, ExprNode::Seq { exprs }),
        signature: Some(crate::lower::typing::fn_sig(
            vec![(Symbol::from("selector"), Ty::Sym)],
            Ty::Untyped,
        )),
        effects: Default::default(),
        enclosing_class: Some(controller.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
        unsupported_formals: None,
        has_anonymous_block: false,
    }
}

/// Route dispatch and the request-test harness use the same adapter selection.
pub(crate) fn request_controllers(controllers: &[Controller]) -> Vec<&Controller> {
    controllers
        .iter()
        .filter(|c| overrides(c, controllers))
        .collect()
}

pub(super) fn inherit_action_tails(
    all_methods: &mut [(Vec<MethodDef>, &Controller)],
    controllers: &[Controller],
    tails: &crate::det::HashMap<crate::ident::ClassId, crate::det::HashMap<Symbol, Expr>>,
) {
    use crate::expr::{Literal, Pattern};
    fn visit(body: &mut Expr, inherited: &crate::det::HashMap<Symbol, Expr>) {
        if let ExprNode::Case { arms, .. } = &mut *body.node {
            for arm in arms {
                let Pattern::Lit { value } = &arm.pattern else {
                    continue;
                };
                let name = match value {
                    Literal::Str { value } => Symbol::from(value.as_str()),
                    Literal::Sym { value } => value.clone(),
                    _ => continue,
                };
                let Some(tail) = inherited.get(&name) else {
                    continue;
                };
                if let ExprNode::Seq { exprs } = &mut *arm.body.node {
                    if exprs.len() == 2
                        && matches!(&*exprs[0].node,
                        ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, .. }
                        if name.as_str() == super::process_action::RESULT)
                    {
                        exprs.insert(1, tail.clone());
                    }
                }
            }
        }
        body.node
            .for_each_child_mut(&mut |child| visit(child, inherited));
    }
    for (methods, controller) in all_methods {
        if may_inline_callbacks(controller, controllers) {
            continue;
        }
        let mut inherited = crate::det::HashMap::default();
        let mut seen: crate::det::HashSet<Symbol> =
            controller.actions().map(|a| a.name.clone()).collect();
        for parent in super::ancestor_chain(controller, controllers).iter().rev() {
            for action in parent.actions() {
                if seen.insert(action.name.clone()) {
                    if let Some(tail) = tails.get(&parent.name).and_then(|ts| ts.get(&action.name))
                    {
                        inherited.insert(action.name.clone(), tail.clone());
                    }
                }
            }
        }
        for method in methods
            .iter_mut()
            .filter(|m| matches!(m.name.as_str(), DISPATCH | "process_action"))
        {
            visit(&mut method.body, &inherited);
        }
    }
}

/// Lift the values of the final action calls, halt returns and rescue handlers
/// into the dispatcher signature. A trailing after callback is not its result.
fn dispatcher_return(body: &Expr) -> crate::ty::Ty {
    use crate::expr::LValue;
    use crate::ty::Ty;
    fn visit(body: &Expr, result: &mut Ty) {
        let value = match &*body.node {
            ExprNode::Assign {
                target: LValue::Var { name, .. },
                value,
            } if name.as_str() == super::process_action::RESULT => Some(value),
            ExprNode::Return { value } => Some(value),
            _ => None,
        };
        if let Some(value) = value {
            *result =
                crate::analyze::union_of(result.clone(), value.ty.clone().unwrap_or(Ty::Untyped));
        }
        if let ExprNode::BeginRescue { rescues, .. } = &*body.node {
            for rescue in rescues {
                *result = crate::analyze::union_of(
                    result.clone(),
                    crate::analyze::effective_return_ty(&rescue.body).unwrap_or(Ty::Untyped),
                );
            }
        }
        body.node.for_each_child(&mut |child| visit(child, result));
    }
    let mut result = Ty::Bottom;
    visit(body, &mut result);
    if result == Ty::Bottom {
        Ty::Nil
    } else {
        result
    }
}

pub(super) fn refresh_return_contracts(
    classes: &mut [LibraryClass],
    controllers: &[Controller],
    registry: &crate::det::HashMap<crate::ident::ClassId, crate::analyze::ClassInfo>,
    ivars: &crate::det::HashMap<Symbol, crate::ty::Ty>,
) {
    use crate::ty::Ty;
    let affected: crate::det::HashSet<_> = controllers
        .iter()
        .filter(|c| !may_inline_callbacks(c, controllers))
        .map(|c| c.name.clone())
        .collect();
    if affected.is_empty() {
        return;
    }
    // Preserve contracts supplied by other lowerings (for example typed
    // parameter objects); refine only the gradual return slots built here.
    let inferred: crate::det::HashSet<_> = classes
        .iter()
        .filter(|c| affected.contains(&c.name))
        .flat_map(|c| {
            c.methods.iter().filter_map(|m| match &m.signature {
                Some(Ty::Fn { ret, .. })
                    if m.receiver == MethodReceiver::Instance && **ret == Ty::Untyped =>
                {
                    Some((c.name.clone(), m.name.clone()))
                }
                _ => None,
            })
        })
        .collect();
    let mut registry = registry.clone();
    let mut ivars = ivars.clone();
    ivars.insert(Symbol::from("body"), Ty::Str);
    ivars.insert(Symbol::from("status"), Ty::Int);
    let limit = classes
        .iter()
        .filter(|c| affected.contains(&c.name))
        .map(|c| c.methods.len())
        .sum::<usize>()
        + 1;
    for _ in 0..limit {
        for class in classes.iter() {
            let info = registry.entry(class.name.clone()).or_default();
            info.parent = class.parent.clone();
            for method in &class.methods {
                if method.receiver == MethodReceiver::Instance {
                    if let Some(sig) = &method.signature {
                        info.instance_methods
                            .insert(method.name.clone(), sig.clone());
                    }
                }
            }
        }
        // A hook on an action-less parent calls a specialization provided
        // by its concrete descendants. Register that virtual contract for
        // typing the parent's body, without emitting a competing method.
        for controller in controllers.iter().filter(|c| overrides(c, controllers)) {
            let own = classes
                .iter()
                .find(|c| c.name == controller.name)
                .and_then(|c| c.methods.iter().find(|m| m.name.as_str() == DISPATCH));
            if own.is_some() {
                continue;
            }
            let mut result = Ty::Bottom;
            for class in classes.iter().filter(|class| {
                controllers
                    .iter()
                    .find(|c| c.name == class.name)
                    .is_some_and(|c| {
                        super::ancestor_chain(c, controllers)
                            .iter()
                            .any(|p| p.name == controller.name)
                    })
            }) {
                if let Some(Ty::Fn { ret, .. }) = class
                    .methods
                    .iter()
                    .find(|m| m.name.as_str() == DISPATCH)
                    .and_then(|m| m.signature.as_ref())
                {
                    result = crate::analyze::union_of(result, (**ret).clone());
                }
            }
            if result != Ty::Bottom {
                registry
                    .entry(controller.name.clone())
                    .or_default()
                    .instance_methods
                    .insert(
                        Symbol::from(DISPATCH),
                        crate::lower::typing::fn_sig(
                            vec![(Symbol::from("action_name"), Ty::Str)],
                            result,
                        ),
                    );
            }
        }
        let mut changed = false;
        for class in classes.iter_mut().filter(|c| affected.contains(&c.name)) {
            for method in &mut class.methods {
                if method.receiver != MethodReceiver::Instance {
                    continue;
                }
                crate::lower::typing::type_method_body(method, &registry, &ivars);
                if !inferred.contains(&(class.name.clone(), method.name.clone())) {
                    continue;
                }
                let result = if method.name.as_str() == DISPATCH
                    || (method.name.as_str() == "process_action"
                        && !overrides(
                            controllers.iter().find(|c| c.name == class.name).unwrap(),
                            controllers,
                        )) {
                    dispatcher_return(&method.body)
                } else {
                    crate::analyze::effective_return_ty(&method.body).unwrap_or(Ty::Untyped)
                };
                if let Some(Ty::Fn { ret, .. }) = &mut method.signature {
                    if **ret != result {
                        *ret = result.into();
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return;
        }
    }
    crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
        crate::span::Span::synthetic(),
        None,
        "process_action_return",
        "controller return contracts did not stabilize",
    ));
}
