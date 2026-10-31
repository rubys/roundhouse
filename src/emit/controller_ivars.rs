//! App controller ivars that a target's emit would merge with the
//! runtime `ActionController::Base`'s own members.

/// An app controller's ivar that shares a name with a member of the
/// runtime `ActionController::Base` takes another name. Ruby keeps
/// `@status` apart from the runtime's own `@status` and its `status`
/// reader: Rails keeps its state in `@_`-prefixed ivars, and an ivar never
/// shares a namespace with a method. In the strict targets the app
/// controller extends the transpiled `Base`, so the two met: a TypeScript
/// class property shares one namespace with the base's fields and methods
/// (`tsc` refused the override, TS2416, #1614), and a Crystal subclass
/// redeclares the base's ivar with its own type (#1625). Even where the
/// types agree, the two would be one value.
///
/// The names come from the runtime's own `Base`: its methods, under every
/// name `method_names` gives the target's class emit for them (TypeScript's
/// `performed?` is `is_performed()`), and the ivars its bodies use, so a
/// runtime change can't leave one out. One rename map
/// covers every app controller, so an ivar ApplicationController sets and a
/// subclass reads keeps one name. The new name (`app_<name>`, numbered if
/// taken) is checked against the runtime's members and every controller's
/// own methods and ivars. Views get the values as arguments, under their
/// own parameter names, so nothing outside the controllers changes. The
/// lowering's own framework ivars (`@params`, `@flash`, `@session`) refer
/// to the runtime's state on purpose and keep their names.
pub(crate) fn rename_ivars_shadowing_controller_base(
    controllers: &mut [crate::dialect::LibraryClass],
    runtime: &[crate::dialect::LibraryClass],
    method_names: impl Fn(&crate::ident::Symbol) -> Vec<crate::ident::Symbol>,
) {
    use crate::expr::{Expr, ExprNode, LValue};
    use crate::ident::Symbol;
    fn ivars_in(e: &Expr, out: &mut crate::det::HashSet<Symbol>) {
        let lvalue = |lv: &LValue, out: &mut crate::det::HashSet<Symbol>| {
            if let LValue::Ivar { name } = lv {
                out.insert(name.clone());
            }
        };
        match &*e.node {
            ExprNode::Ivar { name } => {
                out.insert(name.clone());
            }
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => lvalue(target, out),
            ExprNode::MultiAssign { targets, .. } => targets.iter().for_each(|t| lvalue(t, out)),
            _ => {}
        }
        e.node.for_each_child(&mut |c| ivars_in(c, out));
    }
    fn rename(e: &mut Expr, map: &crate::det::HashMap<Symbol, Symbol>) {
        let lvalue = |lv: &mut LValue| {
            if let LValue::Ivar { name } = lv
                && let Some(to) = map.get(name)
            {
                *name = to.clone();
            }
        };
        match &mut *e.node {
            ExprNode::Ivar { name } => {
                if let Some(to) = map.get(name) {
                    *name = to.clone();
                }
            }
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => lvalue(target),
            ExprNode::MultiAssign { targets, .. } => targets.iter_mut().for_each(lvalue),
            _ => {}
        }
        e.node.for_each_child_mut(&mut |c| rename(c, map));
    }

    const BASE: &str = "ActionController::Base";
    let mut taken: crate::det::HashSet<Symbol> = crate::det::HashSet::default();
    for class in runtime.iter().filter(|c| c.name.0.as_str() == BASE) {
        for m in &class.methods {
            taken.extend(method_names(&m.name));
            ivars_in(&m.body, &mut taken);
        }
    }
    if taken.is_empty() {
        return;
    }
    // A controller is a class whose parents lead to the runtime base.
    let parents: crate::det::HashMap<String, Option<String>> = controllers
        .iter()
        .map(|c| (c.name.0.as_str().to_string(), c.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    let is_controller = |name: &str| {
        let mut at = Some(name.to_string());
        let mut seen = crate::det::HashSet::default();
        while let Some(n) = at {
            if n == BASE {
                return true;
            }
            if !seen.insert(n.clone()) {
                return false;
            }
            at = parents.get(&n).cloned().flatten();
        }
        false
    };
    let controller_idx: Vec<usize> =
        (0..controllers.len()).filter(|&i| is_controller(controllers[i].name.0.as_str())).collect();
    let mut app_ivars: crate::det::HashSet<Symbol> = crate::det::HashSet::default();
    for &i in &controller_idx {
        for m in &controllers[i].methods {
            taken.extend(method_names(&m.name));
            ivars_in(&m.body, &mut app_ivars);
        }
    }
    // The lowering's own `@params` / `@flash` / `@session` ARE the
    // runtime's: they stay.
    let framework = crate::lower::controller_to_library::FRAMEWORK_IVARS;
    let mut shadowing: Vec<Symbol> = app_ivars
        .iter()
        .filter(|n| taken.contains(*n) && !framework.contains(&n.as_str()))
        .cloned()
        .collect();
    shadowing.sort();
    if shadowing.is_empty() {
        return;
    }
    taken.extend(app_ivars.iter().cloned());
    let mut map: crate::det::HashMap<Symbol, Symbol> = crate::det::HashMap::default();
    for name in shadowing {
        let mut n = 1;
        let to = loop {
            let candidate = if n == 1 {
                Symbol::from(format!("app_{}", name.as_str()))
            } else {
                Symbol::from(format!("app_{}_{n}", name.as_str()))
            };
            if !taken.contains(&candidate) {
                break candidate;
            }
            n += 1;
        };
        taken.insert(to.clone());
        map.insert(name, to);
    }
    for &i in &controller_idx {
        for m in &mut controllers[i].methods {
            rename(&mut m.body, &map);
        }
    }
}
