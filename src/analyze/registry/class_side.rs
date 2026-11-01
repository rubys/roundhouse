//! Which app library classes have a class side the registry holds whole
//! (`ClassInfo::class_side_closed`), so that a class-side call none of
//! their tables answers is reported instead of passing as unknown.
//!
//! Library-class bodies are outside `diagnose`'s walk: a miss there is
//! silent by policy, because a gem's DSL kept verbatim in the class body
//! can define anything. A class with no such call, no mixin or ancestor
//! from outside the app and no singleton metaprogramming cannot gain a
//! class method the analyzer does not see. There Ruby raises
//! NoMethodError for `Klass.missing`, and the emitted program does too.

use crate::det::{HashMap, HashSet};

use crate::analyze::ClassInfo;
use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode};
use crate::ident::ClassId;
use crate::App;

/// Calls that add singleton methods out of the analyzer's sight, or hand
/// a block a different `self`.
const SINGLETON_METAPROGRAMMING: &[&str] = &[
    "define_singleton_method",
    "singleton_class",
    "class_eval",
    "class_exec",
    "module_eval",
    "module_exec",
    "instance_eval",
    "instance_exec",
    "extend",
    "method_missing",
    "respond_to_missing?",
    "send",
    "public_send",
    "__send__",
];

/// Mark every closed class. `catalog` is the registry's keys before the
/// app's library classes joined it: an app file reopening a framework or
/// stdlib class is never closed.
pub(in crate::analyze) fn mark(
    classes: &mut HashMap<ClassId, ClassInfo>,
    app: &App,
    catalog: &HashSet<ClassId>,
) {
    let mut declarations: HashMap<&ClassId, Vec<&LibraryClass>> = HashMap::default();
    for lc in &app.library_classes {
        declarations.entry(&lc.name).or_default().push(lc);
    }
    let census = app.gem_lock.as_ref().map(crate::gems::GemCensus::of);
    let own_surface_closed = |id: &ClassId| -> bool {
        let Some(decls) = declarations.get(id) else { return false };
        !catalog.contains(id)
            && !crate::gem_boundary::RUBY_CORE.contains(&id.0.as_str())
            && census.as_ref().is_none_or(|c| crate::gems::gem_defining_constant(c, id.0.as_str()).is_none())
            && classes.get(id).is_some_and(|c| !c.open && !c.gem_boundary)
            && decls.iter().all(|lc| declaration_closed(lc, &declarations))
    };
    let mut closed: HashSet<ClassId> = HashSet::default();
    for id in declarations.keys() {
        // The class and every superclass up to Object.
        let mut current = Some(*id);
        let mut ok = true;
        for _ in 0..32 {
            let Some(cid) = current else { break };
            if !own_surface_closed(cid) {
                ok = false;
                break;
            }
            let parents: HashSet<Option<&ClassId>> =
                declarations[cid].iter().map(|lc| lc.parent.as_ref()).collect();
            current = match parents.into_iter().collect::<Vec<_>>().as_slice() {
                [parent] => *parent,
                _ => {
                    ok = false;
                    break;
                }
            };
        }
        if ok && current.is_none() {
            closed.insert((*id).clone());
        }
    }
    for id in closed {
        if let Some(info) = classes.get_mut(&id) {
            info.class_side_closed = true;
        }
    }
}

/// One declaration: no verbatim class-body call, no template origin, only
/// app modules mixed in (themselves free of class-body calls), and no
/// constant value, class-ivar write or method body reaching for singleton
/// metaprogramming.
fn declaration_closed(lc: &LibraryClass, declarations: &HashMap<&ClassId, Vec<&LibraryClass>>) -> bool {
    let module_ok = |id: &ClassId| {
        declarations.get(id).is_some_and(|decls| {
            decls.iter().all(|m| m.is_module && m.unknown_calls.is_empty() && m.origin.is_none() && methods_closed(m))
        })
    };
    // Constant values and `@ivar` writes run in the class body too, outside `unknown_calls`.
    lc.unknown_calls.is_empty()
        && lc.origin.is_none()
        && lc.constants.iter().all(|(_, value)| !reaches_metaprogramming(value))
        && lc.class_ivar_initializers.iter().all(|value| !reaches_metaprogramming(value))
        && lc.includes.iter().all(module_ok)
        && methods_closed(lc)
}

fn methods_closed(lc: &LibraryClass) -> bool {
    lc.methods.iter().all(|m| {
        !SINGLETON_METAPROGRAMMING.contains(&m.name.as_str())
            && !reaches_metaprogramming(&m.body)
            && m.params.iter().all(|p| p.default.as_ref().is_none_or(|d| !reaches_metaprogramming(d)))
    })
}

fn reaches_metaprogramming(expr: &Expr) -> bool {
    if let ExprNode::Send { method, .. } = &*expr.node {
        if SINGLETON_METAPROGRAMMING.contains(&method.as_str()) {
            return true;
        }
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| found = found || reaches_metaprogramming(child));
    found
}
