//! Tree-shake the framework runtime: filter `LibraryClass.methods`
//! to only the methods the app actually reaches.
//!
//! Algorithm:
//!   1. Build a class registry `name → LibraryClass` covering both
//!      app-side and runtime-side classes.
//!   2. Roots: every `Send` in every app-side method body. Resolve
//!      the receiver's class (typed Sends carry it via `recv.ty`)
//!      and add (class, method_name) to the reachable set.
//!   3. Fixed-point: pop from the work queue, look up the method
//!      definition (walking the parent chain), and walk its body's
//!      Sends. Insert each new pair into the queue.
//!   4. Filter: for each runtime `LibraryClass`, retain only methods
//!      whose name is in the reachable set under that class OR a
//!      descendant class.
//!
//! Conservative on untyped Sends: when `recv.ty` is None / Var /
//! Untyped, fan out — mark the method name reachable on every class
//! that defines it. Loses some shaking opportunity but never wrong.
//!
//! Roots are every app-side method body the caller passes — models,
//! views, controllers, fixtures, AND tests (the emitted test files
//! call app/runtime methods directly, so shaking under them would
//! break the toolchain lanes) — plus app-side standalone functions
//! (seeds, route helpers) and each runtime unit's declared
//! `extra_roots` (calls made by hand-written runtime siblings the
//! body walk can't see). [`Reachability::for_app_units`] packages
//! that contract so every emitter builds the same root set.

use crate::dialect::{LibraryClass, LibraryFunction};
use crate::expr::{Expr, ExprNode, InterpPart, LValue, RescueClause};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;
use std::collections::{HashMap, HashSet, VecDeque};

/// A method-call target: (class that owns/defines or inherits the
/// method, method name). The same method name on different classes
/// is a different reachable entry.
pub type MethodId = (ClassId, Symbol);

/// Reachability set built from app-side roots and the transitive
/// closure of method-body walks.
pub struct Reachability {
    reachable: HashSet<MethodId>,
    /// Method names known to be reachable on at-least-one class.
    /// Used by `is_method_reachable_anywhere` for the conservative
    /// case where a runtime method might be reached via an untyped
    /// Send the typer couldn't narrow.
    reachable_names: HashSet<Symbol>,
}

impl Reachability {
    /// Build the reachable set for an app against parsed runtime
    /// units — the shared entry point every emitter should use.
    ///
    /// `app_classes` must carry EVERY app-side class the target
    /// emits (models, views, controllers, fixtures, tests, test
    /// inner classes): each one's method bodies become roots, and
    /// anything missing here is a class whose calls can't keep
    /// runtime methods alive. `app_functions` carries the app-side
    /// standalone functions (seeds, route helpers). Runtime aliases
    /// (each class under its qualified name AND its last segment)
    /// and the per-unit `extra_roots` are derived here so per-target
    /// call sites can't drift from each other.
    pub fn for_app_units(
        app_classes: &[LibraryClass],
        runtime_units: &[crate::runtime_loader::RuntimeUnit],
        app_functions: &[LibraryFunction],
    ) -> Self {
        // Each runtime LibraryClass registers under its fully-
        // qualified path (`ActiveRecord::Base`) for app-side parent-
        // chain lookups AND under its last segment (`Base`) — the
        // body-typer's Const arm resolves bare app-level references
        // as the simple name, and in-runtime cross-references carry
        // only the simple name. Two `Base` classes (AR and
        // ActionController) collide on the simple name; the registry
        // is a multi-map, so both survive and lookups try each.
        let runtime_aliases: Vec<(ClassId, &LibraryClass)> = runtime_units
            .iter()
            .flat_map(|u| {
                u.classes.iter().flat_map(|c| {
                    let raw = c.name.0.as_str();
                    let mut entries = vec![(c.name.clone(), c)];
                    let last = raw.rsplit("::").next().unwrap_or(raw);
                    if last != raw {
                        entries.push((ClassId(Symbol::from(last)), c));
                    }
                    entries
                })
            })
            .collect();
        // Hand-written runtime files (server, test_support,
        // broadcasts) call into transpiled framework methods the
        // app-body walk can't see; each unit declares those
        // `(class, method)` pairs as extra roots.
        let extra_roots: Vec<(ClassId, Symbol)> = runtime_units
            .iter()
            .flat_map(|u| {
                u.extra_roots
                    .iter()
                    .map(|(cls, method)| (ClassId(Symbol::from(*cls)), Symbol::from(*method)))
            })
            .collect();
        Self::from_app_roots(app_classes, &runtime_aliases, app_functions, &extra_roots)
    }

    /// Build the reachable set from app-side method bodies as roots.
    /// `app_classes` and `runtime_aliases` together form the search
    /// universe.
    ///
    /// `runtime_aliases` is a list of `(name, &LibraryClass)` pairs
    /// — each runtime LibraryClass should appear at least twice:
    /// once under its simple name (`Base`) for in-runtime
    /// cross-references, once under its qualified name
    /// (`ActiveRecord::Base`) for app-side parent-chain lookups.
    /// The caller (typescript.rs) builds these pairs since it
    /// knows each unit's namespace.
    pub fn from_app_roots(
        app_classes: &[LibraryClass],
        runtime_aliases: &[(ClassId, &LibraryClass)],
        app_functions: &[LibraryFunction],
        extra_roots: &[(ClassId, Symbol)],
    ) -> Self {
        // Multi-map: simple names like `Base` collide between
        // ActiveRecord::Base and ActionController::Base. The
        // qualified-name aliases disambiguate parent-chain
        // lookups from app-side classes; for in-runtime
        // cross-references where the typer carries only the
        // simple name, we fall back to "every class with this
        // name" and pick the first that has the method.
        let mut registry: HashMap<ClassId, Vec<&LibraryClass>> = HashMap::new();
        for lc in app_classes {
            registry.entry(lc.name.clone()).or_default().push(lc);
        }
        for (alias, lc) in runtime_aliases {
            registry.entry(alias.clone()).or_default().push(lc);
        }

        let mut reachable: HashSet<MethodId> = HashSet::new();
        let mut reachable_names: HashSet<Symbol> = HashSet::new();
        let mut queue: VecDeque<MethodId> = VecDeque::new();

        let record = |target: MethodId,
                      q: &mut VecDeque<MethodId>,
                      set: &mut HashSet<MethodId>,
                      names: &mut HashSet<Symbol>| {
            names.insert(target.1.clone());
            if set.insert(target.clone()) {
                q.push_back(target);
            }
        };

        // Roots: every Send in every app-side method body, plus
        // every Send in every app-side standalone function body
        // (seeds, route helpers, schema, importmap).
        for app_lc in app_classes {
            for method in &app_lc.methods {
                walk_sends(&method.body, &mut |recv_ty, method_name| {
                    for target in resolve_targets(recv_ty, method_name, &registry) {
                        record(target, &mut queue, &mut reachable, &mut reachable_names);
                    }
                });
            }
        }
        for func in app_functions {
            walk_sends(&func.body, &mut |recv_ty, method_name| {
                for target in resolve_targets(recv_ty, method_name, &registry) {
                    record(target, &mut queue, &mut reachable, &mut reachable_names);
                }
            });
        }

        // Implicit roots: every runtime class's `initialize` and
        // attr_reader/writer methods are always-kept by the filter
        // (constructors invoked via untraceable `new` paths;
        // accessors back ivar/property access at non-Send call
        // sites). Force them into the queue so their bodies get
        // walked for transitive reachability — `Parameters.initialize`
        // body calls `symbolize_keys`, which only reaches the
        // reachable set through this implicit-root pass.
        for (alias, lc) in runtime_aliases {
            for m in &lc.methods {
                use crate::dialect::AccessorKind;
                let always_kept = m.name.as_str() == "initialize"
                    || matches!(
                        m.kind,
                        AccessorKind::AttributeReader | AccessorKind::AttributeWriter
                    );
                if always_kept {
                    record(
                        (alias.clone(), m.name.clone()),
                        &mut queue,
                        &mut reachable,
                        &mut reachable_names,
                    );
                }
            }
        }

        // Hand-written runtime files (server.ts, test_support.ts,
        // broadcasts.ts) call into the transpiled framework
        // (`Router.match`, etc.) — those Sends aren't in the app-side
        // bodies the walker scans, so the methods would otherwise be
        // dropped. The runtime_loader carries an `extra_roots` list
        // per file naming the (class, method) pairs its hand-written
        // siblings reference; we add each one as a root the same way
        // an app-side `Class.method(...)` Send would.
        for (cls, method) in extra_roots {
            let recv_ty = Ty::Class {
                id: cls.clone(),
                args: vec![],
            };
            for target in resolve_targets(Some(&recv_ty), method, &registry) {
                record(target, &mut queue, &mut reachable, &mut reachable_names);
            }
        }

        // Fixed-point: pop, look up method body, walk its Sends.
        while let Some((cls, m)) = queue.pop_front() {
            let resolved = lookup_method(&registry, &cls, &m);
            if let Some((_owner, def)) = resolved {
                walk_sends(&def.body, &mut |recv_ty, method_name| {
                    for target in resolve_targets(recv_ty, method_name, &registry) {
                        record(target, &mut queue, &mut reachable, &mut reachable_names);
                    }
                });
            }
        }

        Reachability {
            reachable,
            reachable_names,
        }
    }

    /// Is the method `<class>#<method_name>` (or static `<class>.<method_name>`)
    /// reachable from app code?
    pub fn contains(&self, class: &ClassId, method: &Symbol) -> bool {
        self.reachable.contains(&(class.clone(), method.clone()))
    }

    /// Conservative fallback: is the method name reachable on
    /// *some* class? Used when we can't precisely determine which
    /// class owns a method (e.g. method whose receiver type wasn't
    /// inferred). Filters less aggressively but never drops something
    /// that's actually called.
    pub fn name_reachable(&self, method: &Symbol) -> bool {
        self.reachable_names.contains(method)
    }

    /// How many distinct (class, method) pairs are reachable.
    /// Useful for diagnostic logging.
    pub fn len(&self) -> usize {
        self.reachable.len()
    }
}

/// Walk an expression tree, calling `visit` on every Send with the
/// receiver's type (if known) and the method name.
fn walk_sends<F>(e: &Expr, visit: &mut F)
where
    F: FnMut(Option<&Ty>, &Symbol),
{
    match &*e.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            let recv_ty = recv.as_ref().and_then(|r| r.ty.as_ref());
            visit(recv_ty, method);
            if let Some(r) = recv {
                walk_sends(r, visit);
            }
            for a in args {
                walk_sends(a, visit);
            }
            if let Some(b) = block {
                walk_sends(b, visit);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            walk_sends(fun, visit);
            for a in args {
                walk_sends(a, visit);
            }
            if let Some(b) = block {
                walk_sends(b, visit);
            }
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                walk_sends(x, visit);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_sends(cond, visit);
            walk_sends(then_branch, visit);
            walk_sends(else_branch, visit);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_sends(left, visit);
            walk_sends(right, visit);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_sends(el, visit);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_sends(k, visit);
                walk_sends(v, visit);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_sends(expr, visit);
                }
            }
        }
        ExprNode::Lambda { body, .. } => walk_sends(body, visit),
        ExprNode::Let { value, body, .. } => {
            walk_sends(value, visit);
            walk_sends(body, visit);
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                walk_sends(recv, visit);
            }
            walk_sends(value, visit);
        }
        ExprNode::Yield { args } => {
            for a in args {
                walk_sends(a, visit);
            }
        }
        ExprNode::Raise { value } => walk_sends(value, visit),
        ExprNode::RescueModifier { expr, fallback } => {
            walk_sends(expr, visit);
            walk_sends(fallback, visit);
        }
        ExprNode::Return { value } => walk_sends(value, visit),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    walk_sends(a, visit);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk_sends(body, visit);
            for r in rescues {
                let RescueClause { classes, body, .. } = r;
                for c in classes {
                    walk_sends(c, visit);
                }
                walk_sends(body, visit);
            }
            if let Some(b) = else_branch {
                walk_sends(b, visit);
            }
            if let Some(b) = ensure {
                walk_sends(b, visit);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                walk_sends(v, visit);
            }
        }
        ExprNode::Splat { value } => walk_sends(value, visit),
        ExprNode::MultiAssign { value, .. } => walk_sends(value, visit),
        ExprNode::While { cond, body, .. } => {
            walk_sends(cond, visit);
            walk_sends(body, visit);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_sends(scrutinee, visit);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_sends(g, visit);
                }
                walk_sends(&arm.body, visit);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk_sends(b, visit);
            }
            if let Some(e) = end {
                walk_sends(e, visit);
            }
        }
        ExprNode::Cast { value, .. } => walk_sends(value, visit),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
    }
}

/// Resolve which (class, method_name) targets a Send hits. Typed
/// Sends produce one target per class candidate (simple-name
/// ambiguity creates multiple); untyped Sends fan out to every
/// class that defines a method with that name.
fn resolve_targets(
    recv_ty: Option<&Ty>,
    method: &Symbol,
    registry: &HashMap<ClassId, Vec<&LibraryClass>>,
) -> Vec<MethodId> {
    match recv_ty {
        Some(Ty::Class { id, .. }) => {
            // Walk parent chain to find the owning class. The
            // reachable set tags BOTH the call-site class (because
            // "Article.method is reachable" covers Article-typed
            // call sites) and the actual owner. Multiple classes
            // may share the same simple name (`Base` from AR and
            // ActionController) — try each, keep targets from any
            // chain that defines the method.
            let mut targets = vec![(id.clone(), method.clone())];
            let candidates: &[&LibraryClass] = registry
                .get(id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            for start in candidates {
                if let Some(found) =
                    walk_inheritance_for_method(start, registry, method)
                {
                    targets.push((found, method.clone()));
                }
            }
            targets
        }
        // Untyped / Var — fan out: mark method reachable on every
        // class that defines it. Conservative.
        _ => registry
            .values()
            .flatten()
            .filter(|lc| lc.methods.iter().any(|m| m.name == *method))
            .map(|lc| (lc.name.clone(), method.clone()))
            .collect(),
    }
}

/// Walk a class's inheritance chain (parent + includes) looking for
/// the first class that defines `method`. Ruby's method dispatch
/// walks both `class < Parent` and `include Mod` mixins; the
/// ancestor chain is `[Self, included_modules_in_reverse_order,
/// Parent, Parent's_includes, ...]`. We approximate by walking
/// includes before parent (matches Ruby's MRO for the simple cases
/// the framework Ruby uses — single include, no diamond) so the
/// `Base includes Validations` case finds Validations methods
/// before continuing up the parent chain.
fn walk_inheritance_for_method(
    start: &LibraryClass,
    registry: &HashMap<ClassId, Vec<&LibraryClass>>,
    method: &Symbol,
) -> Option<ClassId> {
    let mut visited: std::collections::HashSet<ClassId> =
        std::collections::HashSet::new();
    let mut stack: Vec<&LibraryClass> = vec![start];
    while let Some(lc) = stack.pop() {
        if !visited.insert(lc.name.clone()) {
            continue;
        }
        if lc.methods.iter().any(|m| m.name == *method) {
            return Some(lc.name.clone());
        }
        // Visit includes (mixins) before parent — matches Ruby MRO
        // for the single-include shape framework Ruby uses.
        // Reverse-iter so the stack pop order matches source order.
        for inc in lc.includes.iter().rev() {
            if let Some(inc_lc) = registry.get(inc).and_then(|v| v.first()) {
                stack.push(*inc_lc);
            }
        }
        if let Some(parent) = &lc.parent {
            if let Some(parent_lc) = registry.get(parent).and_then(|v| v.first()) {
                stack.push(*parent_lc);
            }
        }
    }
    None
}

/// Look up a method definition by walking the inheritance chain
/// (parent + includes). Returns `(owning_class, method_def)` — the
/// class where the method was actually found.
fn lookup_method<'a>(
    registry: &'a HashMap<ClassId, Vec<&'a LibraryClass>>,
    class: &ClassId,
    method: &Symbol,
) -> Option<(ClassId, &'a crate::dialect::MethodDef)> {
    // Try each class candidate matching this name (simple-name
    // ambiguity), and walk each one's inheritance chain (parent +
    // includes) looking for the method. First match wins.
    let candidates = registry.get(class)?;
    for start in candidates {
        let mut visited: std::collections::HashSet<ClassId> =
            std::collections::HashSet::new();
        let mut stack: Vec<&LibraryClass> = vec![*start];
        while let Some(lc) = stack.pop() {
            if !visited.insert(lc.name.clone()) {
                continue;
            }
            if let Some(m) = lc.methods.iter().find(|m| m.name == *method) {
                return Some((lc.name.clone(), m));
            }
            for inc in lc.includes.iter().rev() {
                if let Some(inc_lc) = registry.get(inc).and_then(|v| v.first()) {
                    stack.push(*inc_lc);
                }
            }
            if let Some(parent) = &lc.parent {
                if let Some(parent_lc) = registry.get(parent).and_then(|v| v.first()) {
                    stack.push(*parent_lc);
                }
            }
        }
    }
    None
}

/// Filter a runtime `LibraryClass.methods` to only methods reachable
/// from app code. Returns the filtered class. Several keep-rules
/// apply (any one is sufficient):
///
///   - attr_reader / attr_writer / attr_accessor synthesized
///     methods stay unconditionally. They're field-shaped and
///     read via ivar/property access (`this.params`), not method
///     Sends — the walker can't see those references.
///   - `initialize` stays unconditionally. Constructors are
///     called via `new` paths the walker can't always trace.
///   - Lifecycle hook no-ops in Base (`before_save`, `after_save`,
///     etc.) stay if `save` / `destroy` are reachable, since their
///     bodies invoke the hooks directly via Sends. (Captured
///     transitively by the walker; no special-case needed.)
///   - Methods precisely reachable on this class.
///   - Methods whose name is reachable anywhere (conservative
///     fallback for untyped Sends).
/// Filter a MODEL class's lowerer-synthesized optional surface to only
/// the methods reachable from app code. `shakeable` is the per-model
/// name set from `lower::model_to_library::schema::
/// shakeable_synthesized_names` — only names in that set are ever
/// dropped, so user-written methods and the load-bearing framework
/// contract (adapter primitives, hydration, lifecycle hooks) are
/// untouchable by construction. A shakeable method survives if it is
/// precisely reachable on this class OR its name is reachable anywhere
/// (the same conservative fallback `filter_runtime_class` uses — an
/// untyped `record.update!(...)` keeps `update!` on every model).
pub fn filter_synthesized_model_methods(
    class: &LibraryClass,
    shakeable: &std::collections::HashSet<Symbol>,
    reach: &Reachability,
) -> LibraryClass {
    let mut filtered = class.clone();
    filtered.methods.retain(|m| {
        if !shakeable.contains(&m.name) {
            return true;
        }
        reach.contains(&class.name, &m.name) || reach.name_reachable(&m.name)
    });
    filtered
}

pub fn filter_runtime_class(class: &LibraryClass, reach: &Reachability) -> LibraryClass {
    use crate::dialect::AccessorKind;
    let mut filtered = class.clone();
    filtered.methods.retain(|m| {
        // Field-shaped accessors stay — they back ivar/property
        // access at the call site, not method Sends.
        if matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            return true;
        }
        // Constructors stay (callers spell as `new <Class>(...)`).
        if m.name.as_str() == "initialize" {
            return true;
        }
        // Precisely reachable on this class.
        if reach.contains(&class.name, &m.name) {
            return true;
        }
        // Conservative: any class's method by this name is
        // reachable. Catches untyped Sends.
        if reach.name_reachable(&m.name) {
            return true;
        }
        false
    });
    filtered
}
