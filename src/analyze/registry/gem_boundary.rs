//! The gem boundary: classes a locked gem's RBI declares, registered
//! so dispatch can answer for them ([`crate::gem_boundary`]).
//!
//! Purely additive. A class the registry already has -- the app's own,
//! the framework catalog's, the stdlib's -- is left exactly as it is:
//! an RBI describes what a gem defines, and a name the app or the
//! catalog also defines is a different (or a better-known) thing that
//! this table has no business overriding. Core classes an RBI merely
//! reopens (`class String`, `module Kernel`) are never registered for
//! the same reason: registering `Object` would turn every "ancestor we
//! don't model" gradual escape into a hard miss.
//!
//! Soundness is by pruning toward `untyped`:
//!
//! * a class name in a signature that resolves to no registered class
//!   becomes `untyped`, never a class nobody defined;
//! * a mixin that resolves to nothing marks the class `open`, so a
//!   method the declared surface lacks is unknown rather than an error;
//! * an app class that inherits a gem class keeps the gradual escape it
//!   always had (see `ClassInfo::gem_boundary`).

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::super::body::ClassInfo;

/// Ruby's own top-level names. An RBI that mentions them is patching or
/// extending the language, not declaring a gem class.
const RUBY_CORE: &[&str] = &[
    "BasicObject", "Object", "Module", "Class", "Kernel", "Comparable", "Enumerable", "Enumerator",
    "Numeric", "Integer", "Float", "Rational", "Complex", "String", "Symbol", "Array", "Hash",
    "Range", "Regexp", "MatchData", "Struct", "Proc", "Method", "UnboundMethod", "Time", "Date",
    "DateTime", "IO", "File", "Dir", "Exception", "StandardError", "RuntimeError", "NilClass",
    "TrueClass", "FalseClass", "Set", "Random", "Process", "Signal", "Math", "Marshal", "GC",
    "ObjectSpace", "Thread", "Mutex", "Fiber", "Ractor", "Data", "Encoding", "Binding",
    "BigDecimal", "Pathname", "StringIO", "URI", "JSON", "Logger", "Singleton", "Forwardable",
];

/// Mixins that add no callable surface a caller could miss: sorbet's
/// own `extend T::Sig`, and the ActiveSupport::Concern marker.
fn is_marker(name: &str) -> bool {
    name.starts_with("T::") || name == "T" || name == "ActiveSupport::Concern"
}

pub(in crate::analyze) fn register(classes: &mut HashMap<ClassId, ClassInfo>, app: &App) {
    if app.gem_boundary.is_empty() {
        return;
    }
    let declared = &app.gem_boundary.classes;

    // Which declarations become classes: not core, not already known.
    let fresh: HashSet<&ClassId> = declared
        .keys()
        .filter(|id| {
            let name = id.0.as_str();
            let top = name.split("::").next().unwrap_or(name);
            !RUBY_CORE.contains(&top) && !classes.contains_key(*id)
        })
        .collect();

    // Names resolve against the union of what was already known and
    // what is being added, so gem classes may refer to each other.
    let known = |id: &ClassId| classes.contains_key(id) || fresh.contains(id);
    let resolve = |written: &ClassId, scope: &str| -> Option<ClassId> {
        // Tapioca writes absolute names (`::Money::Currency`), so the
        // name as written is tried first; a relative name (hand-written
        // shims) then resolves lexically from the class it appears in.
        if known(written) {
            return Some(written.clone());
        }
        let mut parts: Vec<&str> = scope.split("::").filter(|s| !s.is_empty()).collect();
        while !parts.is_empty() {
            let candidate = ClassId(Symbol::new(&format!("{}::{}", parts.join("::"), written.0.as_str())));
            if known(&candidate) {
                return Some(candidate);
            }
            parts.pop();
        }
        None
    };
    let prune = |ty: &Ty, scope: &str| -> Ty { prune_ty(ty, &|id| resolve(id, scope)) };

    // First pass: build every ClassInfo with resolved types and the
    // instance-method surface of each declared mixin.
    let mut built: HashMap<ClassId, ClassInfo> = HashMap::new();
    let mut open: HashSet<ClassId> = HashSet::new();
    for id in &fresh {
        let gem = &declared[*id];
        let scope = id.0.as_str();
        let mut info = ClassInfo { gem_boundary: true, ..ClassInfo::default() };
        for (name, ty) in &gem.instance_methods {
            info.instance_methods.insert(name.clone(), prune(ty, scope));
        }
        for (name, ty) in &gem.class_methods {
            info.class_methods.insert(name.clone(), prune(ty, scope));
        }
        for name in &gem.constants {
            info.constants.insert(name.clone(), Ty::Untyped);
        }
        let mut is_open = gem.dynamic;
        if let Some(parent) = &gem.parent {
            // Unresolved: left as written. Dispatch already reads a
            // named ancestor the registry lacks as "inherited surface we
            // don't model", which is exactly what it is.
            info.parent = Some(resolve(parent, scope).unwrap_or_else(|| parent.clone()));
        }
        for include in &gem.includes {
            match resolve(include, scope) {
                Some(found) => info.includes.push(found),
                None if is_marker(include.0.as_str()) => {}
                None => is_open = true,
            }
        }
        if is_open {
            open.insert((*id).clone());
        }
        built.insert((*id).clone(), info);
    }

    // `extend M` makes M's instance methods this class's class-side
    // surface. Copied from the declared table (a gem module) or the
    // registry (a catalog module); an `M` neither knows leaves the
    // class-side surface unenumerable.
    for id in &fresh {
        let gem = &declared[*id];
        let scope = id.0.as_str();
        for extend in &gem.extends {
            let Some(target) = resolve(extend, scope) else {
                if !is_marker(extend.0.as_str()) {
                    open.insert((*id).clone());
                }
                continue;
            };
            let methods: Vec<(Symbol, Ty)> = match (built.get(&target), classes.get(&target)) {
                (Some(info), _) => info.instance_methods.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                (None, Some(info)) => info.instance_methods.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                (None, None) => Vec::new(),
            };
            if let Some(info) = built.get_mut(*id) {
                for (name, ty) in methods {
                    info.class_methods.entry(name).or_insert(ty);
                }
            }
        }
    }

    // A class that includes an open module is open, transitively.
    loop {
        let mut grew = false;
        for (id, info) in &built {
            if open.contains(id) {
                continue;
            }
            if info.includes.iter().any(|m| open.contains(m)) {
                open.insert(id.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    for (id, mut info) in built {
        info.open = open.contains(&id);
        classes.insert(id, info);
    }

    // A class the catalog or stdlib registers and an RBI also describes
    // (`Rack::Utils`, `Rack::Mime`) gains the RBI's methods it lacks. The
    // registered entry wins wherever both speak, and a class the app
    // itself declares is never touched: its source is the whole truth.
    let app_owned: HashSet<&ClassId> = app
        .models
        .iter()
        .map(|m| &m.name)
        .chain(app.controllers.iter().map(|c| &c.name))
        .chain(app.library_classes.iter().map(|lc| &lc.name))
        .collect();
    let reopened: Vec<&ClassId> = declared
        .keys()
        .filter(|id| !fresh.contains(*id) && !app_owned.contains(*id) && classes.contains_key(*id))
        .filter(|id| {
            let name = id.0.as_str();
            !RUBY_CORE.contains(&name.split("::").next().unwrap_or(name))
        })
        .collect();
    for id in reopened {
        let gem = &declared[id];
        let scope = id.0.as_str();
        let known = |c: &ClassId| classes.contains_key(c);
        let mut add_instance = Vec::new();
        let mut add_class = Vec::new();
        for (name, ty) in &gem.instance_methods {
            add_instance.push((name.clone(), prune_ty(ty, &|c| resolve_in(&known, c, scope))));
        }
        for (name, ty) in &gem.class_methods {
            add_class.push((name.clone(), prune_ty(ty, &|c| resolve_in(&known, c, scope))));
        }
        if let Some(info) = classes.get_mut(id) {
            for (name, ty) in add_instance {
                info.instance_methods.entry(name).or_insert(ty);
            }
            for (name, ty) in add_class {
                info.class_methods.entry(name).or_insert(ty);
            }
        }
    }
}

/// `written` as a registered class, from `scope` outward.
fn resolve_in(known: &dyn Fn(&ClassId) -> bool, written: &ClassId, scope: &str) -> Option<ClassId> {
    if known(written) {
        return Some(written.clone());
    }
    let mut parts: Vec<&str> = scope.split("::").filter(|s| !s.is_empty()).collect();
    while !parts.is_empty() {
        let candidate = ClassId(Symbol::new(&format!("{}::{}", parts.join("::"), written.0.as_str())));
        if known(&candidate) {
            return Some(candidate);
        }
        parts.pop();
    }
    None
}

/// `ty` with every class name resolved through `resolve`; a name that
/// resolves to nothing becomes `untyped`.
fn prune_ty(ty: &Ty, resolve: &dyn Fn(&ClassId) -> Option<ClassId>) -> Ty {
    match ty {
        Ty::Class { id, args } => match resolve(id) {
            Some(id) => Ty::Class {
                id,
                args: args.iter().map(|a| prune_ty(a, resolve)).collect(),
            },
            None => Ty::Untyped,
        },
        Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(prune_ty(elem, resolve)) },
        Ty::Hash { key, value } => Ty::Hash {
            key: std::sync::Arc::new(prune_ty(key, resolve)),
            value: std::sync::Arc::new(prune_ty(value, resolve)),
        },
        Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(|t| prune_ty(t, resolve)).collect() },
        Ty::Union { variants } => {
            Ty::Union { variants: variants.iter().map(|t| prune_ty(t, resolve)).collect() }
        }
        Ty::Fn { params, block, ret, effects } => Ty::Fn {
            params: params
                .iter()
                .map(|p| crate::ty::Param { name: p.name.clone(), ty: prune_ty(&p.ty, resolve).into(), kind: p.kind.clone() })
                .collect(),
            block: block.as_ref().map(|b| std::sync::Arc::new(prune_ty(b, resolve))),
            ret: std::sync::Arc::new(prune_ty(ret, resolve)),
            effects: effects.clone(),
        },
        other => other.clone(),
    }
}
