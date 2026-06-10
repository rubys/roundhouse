//! `Expr` → Swift source.
//!
//! Phase 2 coverage: the node kinds the lowered model bodies exercise.
//! Ported from `src/emit/kotlin/expr.rs` (the template) with the Swift
//! deltas — `\(...)` interpolation, `??` for nil-coalescing `||`,
//! `switch` for `case`, trailing closures for blocks, `let`/`var`
//! inference for local assignments, `as!` downcasts where Kotlin chained
//! `.toString().toLong()` (see `docs/swift-migration-plan.md` delta 6),
//! and `fatalError(...)` raise placeholders until the Phase 3
//! `throws`-propagation pass exists.
//!
//! Untyped/edge nodes that don't map cleanly emit a `/* TODO kind */`
//! marker rather than panicking, so a full model still renders and the
//! gaps are visible in the output.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::expr::{Arm, BoolOpKind, Expr, ExprNode, InterpPart, LValue, Literal, OpAssignOp, Pattern};

use super::naming::camel;
use super::ty::swift_ty;

thread_local! {
    /// Local names already declared in the current method body (so the
    /// first `Assign` emits `let`/`var` and later ones emit bare `=`).
    static DECLARED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Local names assigned more than once → declared `var` (else `let`).
    static REASSIGNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// For locals first assigned `nil`, the optional Swift type taken
    /// from a later non-nil assignment — so `var x = nil` (which Swift
    /// rejects outright: nil needs a contextual type) becomes
    /// `var x: T? = nil`.
    static NIL_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// camelCased property name → declared `Ty` for the class currently
    /// being emitted. Drives the self-receiver column-write coercion: the
    /// lowerer skips `Cast` insertion on untyped-map → typed-property
    /// assigns for soft targets (`assign_from_row`/`update`), so the
    /// emitter inserts the `as!` downcast — same fix as Kotlin's
    /// INSTANCE_PROP_TYPES cluster.
    static INSTANCE_PROP_TYPES: RefCell<HashMap<String, crate::ty::Ty>> =
        RefCell::new(HashMap::new());
    /// Swift type name → its instance METHOD names (camelCased). A
    /// zero-arg send to a receiver of a known class type keeps its call
    /// parens when the name is a real method (`article.comments()`), vs
    /// the default property read (`article.title`) — Kotlin's
    /// CLASS_INSTANCE_METHODS registry.
    static CLASS_INSTANCE_METHODS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// Whether the method being emitted returns a value — decides
    /// `return nil` vs bare `return` for Ruby's `return nil`.
    static RETURNS_VALUE: RefCell<bool> = const { RefCell::new(false) };
    /// Whether the class being emitted is an Error-conforming Ruby error
    /// class — redirects `super(msg)` in its init to the synthesized
    /// `message` property.
    static IN_ERROR_CLASS: RefCell<bool> = const { RefCell::new(false) };
    /// Whether the method being emitted is an `init` of a parented
    /// class — `super(args)` becomes the designated `super.init(args)`.
    static INIT_SUPER: RefCell<bool> = const { RefCell::new(false) };
    /// "Type.prop" keys for module/object-level accessors whose reads
    /// are property accesses, not calls (`ActiveRecord.adapter`).
    static OBJECT_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// "Type.method" keys (camelCased) for methods marked `throws` by
    /// the raise classification — call sites prefix `try`.
    static THROWS_METHODS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Class → parent (Swift type names), for ancestor walks (inherited
    /// statics like `Article.find` → `ActiveRecordBase.find`).
    static CLASS_PARENTS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Swift type name → its CLASS-method names (camelCased) mapped to
    /// their rendered return types — drives `override class func`
    /// marking on subclass redeclarations (covariant CLASS returns
    /// override legally; covariant CONTAINER returns can only shadow).
    static CLASS_STATIC_METHODS: RefCell<HashMap<String, HashMap<String, String>>> =
        RefCell::new(HashMap::new());
    /// Swift type name → its stored-property names (accessors + body
    /// ivars + collapsed pure readers) — subclasses skip re-declaring
    /// inherited slots, and self-sends to ancestor props read without
    /// parens.
    static CLASS_PROPS: RefCell<HashMap<String, HashSet<String>>> = RefCell::new(HashMap::new());
    /// The class currently being emitted (for ancestor-aware self-send
    /// resolution).
    static CURRENT_CLASS: RefCell<String> = RefCell::new(String::new());
    /// Whether a module enum is being emitted — its `@ivar` state lives
    /// in `static var`s, and static funcs have no `self`, so ivar
    /// assigns emit bare names.
    static IN_MODULE: RefCell<bool> = const { RefCell::new(false) };
    /// Locals to hoist as typed `var` declarations at the method top —
    /// first assigned inside a nested scope but used/reassigned later
    /// (Kotlin's scan_hoist).
    static HOISTED: RefCell<Vec<(String, String, String)>> = const { RefCell::new(Vec::new()) };
    /// Optional properties proven non-nil by the enclosing branch's
    /// nil-guard — reads force-unwrap (Kotlin's `!!` smart-cast
    /// cluster, Swift's `!`).
    static NONNULL_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The current method's parameter names — the view lowerer renders a
    /// partial local as a bare Send in arg position but a Var as a
    /// receiver; a bare zero-arg send naming a param emits the
    /// identifier, not a call.
    static PARAM_NAMES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The current method's parameter types (camelCased name → Ty) —
    /// the optionality fallback when a Var read carries no stamped ty.
    static PARAM_TYPES: RefCell<HashMap<String, crate::ty::Ty>> = RefCell::new(HashMap::new());
    /// "Receiver.method" → ORDERED camelCased param names. Decides
    /// whether a call-site `kwargs: true` hash splats positionally into
    /// the callee's parameter order (Swift funcs here are
    /// underscore-labeled, so named args don't apply).
    static METHOD_PARAMS: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    /// Error-conforming class names — a `raise` of one becomes a real
    /// `throw`; anything else stays `fatalError`.
    static ERROR_CLASSES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Empty-container locals' inferred declaration types, from how
    /// they're later populated (`map[k] = v`, `list << x`) — Kotlin's
    /// CONTAINER_TYPES scan.
    static CONTAINER_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Reset cross-class emit state. Called once at `swift::emit` start.
pub(super) fn reset_registries() {
    INSTANCE_PROP_TYPES.with(|m| m.borrow_mut().clear());
    CLASS_INSTANCE_METHODS.with(|m| m.borrow_mut().clear());
    OBJECT_PROPS.with(|m| m.borrow_mut().clear());
    THROWS_METHODS.with(|m| m.borrow_mut().clear());
    CLASS_PARENTS.with(|m| m.borrow_mut().clear());
    ERROR_CLASSES.with(|m| m.borrow_mut().clear());
    METHOD_PARAMS.with(|m| m.borrow_mut().clear());
}

/// Register a module/object-level accessor property ("Type.prop") whose
/// reads must NOT carry call parens (`ActiveRecord.adapter`).
pub(super) fn register_object_prop(key: String) {
    OBJECT_PROPS.with(|m| {
        m.borrow_mut().insert(key);
    });
}

/// Register a class's parent for ancestor walks.
pub(super) fn register_class_parent(class: String, parent: String) {
    CLASS_PARENTS.with(|m| {
        m.borrow_mut().insert(class, parent);
    });
}

/// Register an Error-conforming class (a `< StandardError` transpile).
pub(super) fn register_error_class(name: String) {
    ERROR_CLASSES.with(|m| {
        m.borrow_mut().insert(name);
    });
}

/// Register a throwing method ("Type.method", camelCased).
pub(super) fn register_throws(key: String) {
    THROWS_METHODS.with(|m| {
        m.borrow_mut().insert(key);
    });
}

/// Does `type.method` (or an ancestor's) throw?
pub(super) fn throws_lookup(type_name: &str, method_camel: &str) -> bool {
    let mut cur = type_name.to_string();
    loop {
        let key = format!("{cur}.{method_camel}");
        if THROWS_METHODS.with(|m| m.borrow().contains(&key)) {
            return true;
        }
        match CLASS_PARENTS.with(|m| m.borrow().get(&cur).cloned()) {
            Some(p) => cur = p,
            None => return false,
        }
    }
}

fn is_error_class_name(name: &str) -> bool {
    ERROR_CLASSES.with(|m| m.borrow().contains(name))
}

/// Flag the class being emitted as an Error-conforming error class.
pub(super) fn set_error_class(flag: bool) {
    IN_ERROR_CLASS.with(|f| *f.borrow_mut() = flag);
}

/// Flag the method being emitted as the init of a parented class.
pub(super) fn set_init_super(flag: bool) {
    INIT_SUPER.with(|f| *f.borrow_mut() = flag);
}

/// Flag module-enum emission (bare ivar assigns, static-var state).
pub(super) fn set_in_module(flag: bool) {
    IN_MODULE.with(|f| *f.borrow_mut() = flag);
}

/// Is this (camelCased) local marked var-requiring by the pre-scan?
/// Drives the `var x = x` shadow for mutated params.
pub(super) fn is_reassigned(name: &str) -> bool {
    REASSIGNED.with(|r| r.borrow().contains(name))
}

/// Mark a name as already declared (param shadows, hoisted vars).
pub(super) fn declare_local(name: &str) {
    DECLARED.with(|d| {
        d.borrow_mut().insert(name.to_string());
    });
}

/// The hoisted-var declarations for the method just begun:
/// `(name, swift_ty, default)` triples.
pub(super) fn take_hoisted() -> Vec<(String, String, String)> {
    HOISTED.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

/// Install the current method's parameter names + types (see
/// `PARAM_NAMES` / `PARAM_TYPES`).
pub(super) fn set_param_names(params: Vec<(String, Option<crate::ty::Ty>)>) {
    PARAM_NAMES.with(|p| *p.borrow_mut() = params.iter().map(|(n, _)| n.clone()).collect());
    PARAM_TYPES.with(|p| {
        *p.borrow_mut() =
            params.into_iter().filter_map(|(n, t)| t.map(|t| (n, t))).collect()
    });
}

fn is_param(method: &str) -> bool {
    PARAM_NAMES.with(|p| p.borrow().contains(&camel(method)))
}

/// Register a callable's ordered parameter names ("Receiver.method").
pub(super) fn register_method_params(key: String, params: Vec<String>) {
    METHOD_PARAMS.with(|m| {
        m.borrow_mut().insert(key, params);
    });
}

fn method_params_for(receiver: &str, method: &str) -> Option<Vec<String>> {
    METHOD_PARAMS.with(|m| m.borrow().get(&format!("{receiver}.{}", camel(method))).cloned())
}

/// Render a call's arguments. A trailing `kwargs: true` hash splats
/// POSITIONALLY into the callee's registered parameter order
/// (`truncate(body, length: 100)` → `truncate(body, 100)`); every kwarg
/// must land on a tail parameter or the hash stays a map literal — so a
/// genuine sym-keyed map arg (an unregistered primitive like
/// `Broadcasts.append`) is never miscaptured.
fn emit_call_args(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    if let Some((last, head)) = args.split_last() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            if !entries.is_empty() {
                let keys: Option<Vec<String>> = entries
                    .iter()
                    .map(|(k, _)| match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            Some(camel(value.as_str()))
                        }
                        _ => None,
                    })
                    .collect();
                let recv_type = recv.and_then(|r| match &*r.node {
                    ExprNode::Const { path } => Some(super::naming::type_name(
                        &path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"),
                    )),
                    _ => None,
                });
                if let (Some(keys), Some(rt)) = (keys, recv_type) {
                    if let Some(params) = method_params_for(&rt, method) {
                        let tail = &params[head.len().min(params.len())..];
                        let mut parts: Vec<String> = head.iter().map(emit_expr).collect();
                        let mut consumed = 0;
                        for p in tail {
                            if let Some(idx) = keys.iter().position(|k| k == p) {
                                parts.push(emit_expr(&entries[idx].1));
                                consumed += 1;
                            } else {
                                break;
                            }
                        }
                        if consumed == keys.len() {
                            return parts.join(", ");
                        }
                    }
                }
            }
        }
    }
    args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
}

/// Register a class's instance-method name set (camelCased).
pub(super) fn register_class_methods(class: String, methods: HashSet<String>) {
    CLASS_INSTANCE_METHODS.with(|m| {
        m.borrow_mut().insert(class, methods);
    });
}

/// Register a class's class-method names (camelCased) → rendered return
/// types.
pub(super) fn register_static_methods(class: String, methods: HashMap<String, String>) {
    CLASS_STATIC_METHODS.with(|m| {
        m.borrow_mut().insert(class, methods);
    });
}

/// The nearest ancestor's return type for a class method, if any
/// ancestor declares it.
pub(super) fn ancestor_static_ret(class: &str, name: &str) -> Option<String> {
    let mut cur = CLASS_PARENTS.with(|m| m.borrow().get(class).cloned());
    while let Some(c) = cur {
        if let Some(ret) =
            CLASS_STATIC_METHODS.with(|m| m.borrow().get(&c).and_then(|s| s.get(name).cloned()))
        {
            return Some(ret);
        }
        cur = CLASS_PARENTS.with(|m| m.borrow().get(&c).cloned());
    }
    None
}

/// Is `sub` the same class as — or a registered descendant of — `sup`?
/// (Both are rendered Swift type names; trailing `?` strips, so
/// optional-covariant overrides resolve too.)
pub(super) fn is_same_or_descendant(sub: &str, sup: &str) -> bool {
    let sub = sub.trim_end_matches('?');
    let sup = sup.trim_end_matches('?');
    let mut cur = Some(sub.to_string());
    while let Some(c) = cur {
        if c == sup {
            return true;
        }
        cur = CLASS_PARENTS.with(|m| m.borrow().get(&c).cloned());
    }
    false
}

/// Register a class's stored-property name set (camelCased).
pub(super) fn register_class_props(class: String, props: HashSet<String>) {
    CLASS_PROPS.with(|m| {
        m.borrow_mut().insert(class, props);
    });
}

/// Set the class currently being emitted.
pub(super) fn set_current_class(name: &str) {
    CURRENT_CLASS.with(|c| *c.borrow_mut() = name.to_string());
}

/// Is `name` a stored property anywhere in the ANCESTOR chain of
/// `class` (excluding the class itself)?
pub(super) fn ancestor_has_prop(class: &str, name: &str) -> bool {
    let mut cur = CLASS_PARENTS.with(|m| m.borrow().get(class).cloned());
    while let Some(c) = cur {
        if CLASS_PROPS.with(|m| m.borrow().get(&c).map_or(false, |s| s.contains(name))) {
            return true;
        }
        cur = CLASS_PARENTS.with(|m| m.borrow().get(&c).cloned());
    }
    false
}

/// Union of a member-name kind across the receiver's ANCESTORS (parent
/// chain, excluding the class itself) — decides `override` marking.
pub(super) fn ancestor_has(class: &str, name: &str, statics: bool) -> bool {
    if statics {
        return ancestor_static_ret(class, name).is_some();
    }
    let mut cur = CLASS_PARENTS.with(|m| m.borrow().get(class).cloned());
    while let Some(c) = cur {
        let hit =
            CLASS_INSTANCE_METHODS.with(|m| m.borrow().get(&c).map_or(false, |s| s.contains(name)));
        if hit {
            return true;
        }
        cur = CLASS_PARENTS.with(|m| m.borrow().get(&c).cloned());
    }
    false
}

fn is_known_instance_method(recv: &Expr, method: &str) -> bool {
    let Some(crate::ty::Ty::Class { id, .. }) = recv.ty.as_ref() else {
        return false;
    };
    let cls = super::naming::type_name(id.0.as_str());
    let name = camel(method);
    CLASS_INSTANCE_METHODS.with(|m| {
        m.borrow().get(&cls).map_or(false, |s| s.contains(&name))
    })
}

/// Install the property-type map for the class about to be emitted.
/// Called by `library::emit_library_class`.
pub(super) fn set_instance_prop_types(map: HashMap<String, crate::ty::Ty>) {
    INSTANCE_PROP_TYPES.with(|m| *m.borrow_mut() = map);
}

/// Coerce a value assigned to a self-receiver property whose declared
/// type is a scalar, when the value's static type doesn't already prove
/// it (untyped map reads). Skips values that already carry a `Cast`.
fn coerce_for_prop_assign(recv: &Expr, prop_raw: &str, value: &Expr, val: String) -> String {
    use crate::ty::Ty;
    if !matches!(&*recv.node, ExprNode::SelfRef) {
        return val;
    }
    coerce_for_prop(prop_raw, value, val)
}

fn coerce_for_prop(prop_raw: &str, value: &Expr, val: String) -> String {
    use crate::ty::Ty;
    let n = camel(prop_raw);
    let Some(ty) = INSTANCE_PROP_TYPES.with(|m| m.borrow().get(&n).cloned()) else {
        return val;
    };
    if matches!(&*value.node, ExprNode::Cast { .. }) {
        return val;
    }
    // The decision keys off the EMITTED surface type, not the IR's
    // belief: a `[String: Any?]` index read surfaces as `Any??` in Swift
    // even when the lowerer has stamped the slot's `Ty` (which is exactly
    // why it inserted no `Cast` for this soft target).
    let surface_untrusted = is_map_read_shape(value)
        || match value.ty.as_ref() {
            None => true,
            Some(t) => matches!(t, Ty::Untyped | Ty::Var { .. }),
        };
    if !surface_untrusted {
        return val;
    }
    match ty {
        Ty::Int => format!("({val} as! Int)"),
        Ty::Float => format!("({val} as! Double)"),
        Ty::Str | Ty::Sym => format!("({val} as! String)"),
        Ty::Bool => format!("({val} as! Bool)"),
        _ => val,
    }
}

/// Is the receiver statically a Hash (directly or through a nullable
/// Union / the declared prop type)?
fn recv_is_hash(r: &Expr) -> bool {
    fn ty_is_hash(t: &crate::ty::Ty) -> bool {
        match t {
            crate::ty::Ty::Hash { .. } => true,
            crate::ty::Ty::Class { id, .. } => id.0.as_str() == "Hash",
            crate::ty::Ty::Union { variants } => variants
                .iter()
                .any(|v| !matches!(v, crate::ty::Ty::Nil) && ty_is_hash(v)),
            _ => false,
        }
    }
    if r.ty.as_ref().map_or(false, ty_is_hash) {
        return true;
    }
    // An ivar read takes the declared property type.
    if let ExprNode::Ivar { name } = &*r.node {
        let n = camel(name.as_str());
        return INSTANCE_PROP_TYPES.with(|m| m.borrow().get(&n).map_or(false, ty_is_hash));
    }
    false
}

/// A value whose Swift surface type is `Any?`-ish regardless of IR
/// stamping: a map index read / fetch, or a `??`-coalesce over one.
fn is_map_read_shape(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { method, .. } if method.as_str() == "[]" || method.as_str() == "fetch" => {
            true
        }
        ExprNode::BoolOp { op: BoolOpKind::Or, left, .. } => is_map_read_shape(left),
        _ => false,
    }
}

/// Reset per-method local-decl tracking and pre-scan the body for
/// reassignment counts. Called by `library::emit_method` before the body
/// is rendered.
pub(super) fn begin_method(body: &Expr, returns_value: bool) {
    RETURNS_VALUE.with(|r| *r.borrow_mut() = returns_value);
    let mut container_types: HashMap<String, String> = HashMap::new();
    scan_container_types(body, &mut container_types);
    CONTAINER_TYPES.with(|t| *t.borrow_mut() = container_types);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut nil_types: HashMap<String, String> = HashMap::new();
    let mut mutated: HashSet<String> = HashSet::new();
    count_assigns(body, &mut counts, &mut nil_types, &mut mutated);
    DECLARED.with(|d| d.borrow_mut().clear());

    // Var-hoist (Kotlin's scan_hoist): a local FIRST assigned inside a
    // nested scope but assigned more than once needs a typed `var`
    // declaration at the method top — Swift scopes the nested decl to
    // its branch.
    let mut hoist_info: HashMap<String, (usize, usize, Option<crate::ty::Ty>)> = HashMap::new();
    scan_hoist(body, 0, &mut hoist_info);
    let mut hoisted: Vec<(String, String, String)> = Vec::new();
    for (n, (first_depth, count, ty)) in hoist_info {
        if first_depth > 0 && count > 1 {
            let (st, d) = hoist_decl(ty.as_ref());
            DECLARED.with(|dset| {
                dset.borrow_mut().insert(n.clone());
            });
            hoisted.push((n, st, d));
        }
    }
    hoisted.sort();
    HOISTED.with(|h| *h.borrow_mut() = hoisted);
    REASSIGNED.with(|r| {
        let mut set = r.borrow_mut();
        set.clear();
        for (name, n) in counts {
            if n > 1 {
                set.insert(name);
            }
        }
        // Swift arrays/dictionaries are VALUE types: calling a mutating
        // member (`append`, index-assign, …) on a `let` local is a
        // compile error, so in-place mutation forces `var` even for a
        // single-assignment local. (No Kotlin analog — MutableList is a
        // reference.)
        set.extend(mutated);
    });
    NIL_TYPES.with(|t| *t.borrow_mut() = nil_types);
}

/// Infer declaration types for empty-container locals from how they're
/// later populated: `map[k] = v` → `[K: V]`, `list << x` → `[E]` —
/// Kotlin's CONTAINER_TYPES scan. Index reads are typed nilable by the
/// IR (Ruby OOB → nil), so the top-level nullability strips.
fn scan_container_types(e: &Expr, out: &mut HashMap<String, String>) {
    let nn = |ty: Option<&crate::ty::Ty>| -> String {
        match ty {
            Some(crate::ty::Ty::Union { variants }) => {
                let non_nil: Vec<&crate::ty::Ty> =
                    variants.iter().filter(|t| !matches!(t, crate::ty::Ty::Nil)).collect();
                if non_nil.len() == 1 {
                    swift_ty(non_nil[0])
                } else {
                    "Any?".to_string()
                }
            }
            Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Var { .. }) | None => {
                "Any?".to_string()
            }
            Some(t) => swift_ty(t),
        }
    };
    match &*e.node {
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => {
            let target = match &*recv.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name),
                _ => None,
            };
            if let Some(name) = target {
                out.entry(camel(name.as_str())).or_insert(format!(
                    "[{}: {}]",
                    nn(index.ty.as_ref()),
                    nn(value.ty.as_ref())
                ));
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            let target = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name),
                _ => None,
            };
            if let Some(name) = target {
                out.entry(camel(name.as_str())).or_insert(format!(
                    "[{}: {}]",
                    nn(args[0].ty.as_ref()),
                    nn(args[1].ty.as_ref())
                ));
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if matches!(method.as_str(), "<<" | "push" | "append") && args.len() == 1 =>
        {
            let target = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name),
                _ => None,
            };
            if let Some(name) = target {
                out.entry(camel(name.as_str()))
                    .or_insert(format!("[{}]", nn(args[0].ty.as_ref())));
            }
        }
        _ => {}
    }
    for child in children(e) {
        scan_container_types(child, out);
    }
}

/// One-off container scan over a body (module-ivar typing).
pub(super) fn container_scan(e: &Expr) -> HashMap<String, String> {
    let mut out = HashMap::new();
    scan_container_types(e, &mut out);
    out
}

/// Hoist-scan walk: branch bodies (If/While/Case/Lambda) are depth+1,
/// Seq and conditions stay at the current depth.
fn scan_hoist(
    e: &Expr,
    depth: usize,
    info: &mut HashMap<String, (usize, usize, Option<crate::ty::Ty>)>,
) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            let cn = camel(name.as_str());
            let entry = info.entry(cn).or_insert((depth, 0, None));
            entry.1 += 1;
            if entry.2.is_none() {
                if let Some(t) = value.ty.as_ref() {
                    if !matches!(t, crate::ty::Ty::Nil) {
                        entry.2 = Some(t.clone());
                    }
                }
            }
            scan_hoist(value, depth, info);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                scan_hoist(x, depth, info);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            scan_hoist(cond, depth, info);
            scan_hoist(then_branch, depth + 1, info);
            scan_hoist(else_branch, depth + 1, info);
        }
        ExprNode::While { cond, body, .. } => {
            scan_hoist(cond, depth, info);
            scan_hoist(body, depth + 1, info);
        }
        ExprNode::Case { scrutinee, arms } => {
            scan_hoist(scrutinee, depth, info);
            for a in arms {
                scan_hoist(&a.body, depth + 1, info);
            }
        }
        ExprNode::Lambda { body, .. } => scan_hoist(body, depth + 1, info),
        _ => {
            for c in children(e) {
                scan_hoist(c, depth, info);
            }
        }
    }
}

/// Declaration type + default for a hoisted local.
fn hoist_decl(ty: Option<&crate::ty::Ty>) -> (String, String) {
    use crate::ty::Ty;
    let Some(t) = ty else {
        return ("Any?".to_string(), "nil".to_string());
    };
    let d = match t {
        Ty::Int => "0",
        Ty::Float => "0.0",
        Ty::Bool => "false",
        Ty::Str | Ty::Sym => "\"\"",
        Ty::Array { .. } => "[]",
        Ty::Hash { .. } => "[:]",
        _ => {
            let mut st = swift_ty(t);
            if !st.ends_with('?') {
                st.push('?');
            }
            return (st, "nil".to_string());
        }
    };
    (swift_ty(t), d.to_string())
}

/// Ruby methods that lower to mutating Swift members on value types.
fn is_mutating_method(m: &str) -> bool {
    matches!(
        m,
        "<<" | "[]="
            | "append"
            | "push"
            | "insert"
            | "delete"
            | "delete_at"
            | "clear"
            | "concat"
            | "merge!"
            | "sort!"
            | "uniq!"
            | "shift"
            | "pop"
            | "unshift"
    )
}

fn count_assigns(
    e: &Expr,
    counts: &mut HashMap<String, usize>,
    nil_types: &mut HashMap<String, String>,
    mutated: &mut HashSet<String>,
) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            let cn = camel(name.as_str());
            *counts.entry(cn.clone()).or_insert(0) += 1;
            // Record the first non-nil assigned type so a `nil`-first
            // local gets a real optional declaration type.
            if !nil_types.contains_key(&cn) {
                if let Some(ty) = value.ty.as_ref() {
                    if !matches!(ty, crate::ty::Ty::Nil) {
                        let mut st = swift_ty(ty);
                        if !st.ends_with('?') {
                            st.push('?');
                        }
                        nil_types.insert(cn, st);
                    }
                }
            }
        }
        ExprNode::Assign { target: LValue::Index { recv, .. }, .. } => {
            if let ExprNode::Var { name, .. } = &*recv.node {
                mutated.insert(camel(name.as_str()));
            }
        }
        // A compound assignment both mutates and (for the count) reassigns.
        ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => {
            mutated.insert(camel(name.as_str()));
        }
        ExprNode::Send { recv: Some(r), method, .. } if is_mutating_method(method.as_str()) => {
            if let ExprNode::Var { name, .. } = &*r.node {
                mutated.insert(camel(name.as_str()));
            }
        }
        _ => {}
    }
    for child in children(e) {
        count_assigns(child, counts, nil_types, mutated);
    }
}

fn emit_op_assign(target: &LValue, op: OpAssignOp, value: &Expr) -> String {
    let t = match target {
        LValue::Var { name, .. } => camel(name.as_str()),
        LValue::Ivar { name } => format!("self.{}", camel(name.as_str())),
        LValue::Attr { recv, name } => {
            format!("{}.{}", emit_expr(recv), camel(name.as_str()))
        }
        LValue::Index { recv, index } => {
            format!("{}[{}]", emit_expr(recv), emit_expr(index))
        }
        LValue::Const { path } => path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("."),
    };
    let v = emit_expr(value);
    match op {
        OpAssignOp::OrOr => format!("{t} = {t} ?? {v}"),
        OpAssignOp::AndAnd => format!("if {t} != nil {{ {t} = {v} }}"),
        OpAssignOp::Add => format!("{t} += {v}"),
        OpAssignOp::Sub => format!("{t} -= {v}"),
        OpAssignOp::Mul => format!("{t} *= {v}"),
        OpAssignOp::Div => format!("{t} /= {v}"),
        OpAssignOp::Mod => format!("{t} %= {v}"),
        OpAssignOp::Pow => format!("{t} = pow({t}, {v})"),
        OpAssignOp::BitAnd => format!("{t} &= {v}"),
        OpAssignOp::BitOr => format!("{t} |= {v}"),
        OpAssignOp::BitXor => format!("{t} ^= {v}"),
        OpAssignOp::Shl => format!("{t} <<= {v}"),
        OpAssignOp::Shr => format!("{t} >>= {v}"),
    }
}

/// Shallow child-expression walk — enough for the assignment pre-scan.
fn children(e: &Expr) -> Vec<&Expr> {
    let mut v = Vec::new();
    match &*e.node {
        ExprNode::Seq { exprs } => v.extend(exprs.iter()),
        ExprNode::If { cond, then_branch, else_branch } => {
            v.push(cond);
            v.push(then_branch);
            v.push(else_branch);
        }
        ExprNode::While { cond, body, .. } => {
            v.push(cond);
            v.push(body);
        }
        ExprNode::Assign { value, .. } => v.push(value),
        ExprNode::Case { scrutinee, arms } => {
            v.push(scrutinee);
            for a in arms {
                v.push(&a.body);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                v.push(r);
            }
            v.extend(args.iter());
            if let Some(b) = block {
                v.push(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            v.push(left);
            v.push(right);
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => v.push(value),
        ExprNode::Lambda { body, .. } => v.push(body),
        _ => {}
    }
    v
}

pub fn emit_expr(e: &Expr) -> String {
    if let Some(s) = try_string_builder(e) {
        return s;
    }
    emit_node(&e.node, e)
}

/// The view lowerer's StringBuilder IrHints. Swift's spelling is plain
/// string accumulation — `var io = ""` / `io += chunk` / `io` (String
/// append is amortized O(1)); mirrors `kotlin::expr::try_string_builder`.
/// Non-hinted sites fall through to the normal walkers.
fn try_string_builder(e: &Expr) -> Option<String> {
    match e.hint? {
        crate::expr::IrHint::StringBuilderInit => {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                let n = camel(name.as_str());
                DECLARED.with(|d| {
                    d.borrow_mut().insert(n.clone());
                });
                return Some(format!("var {n} = \"\""));
            }
            None
        }
        crate::expr::IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        return Some(format!(
                            "{} += {}",
                            camel(name.as_str()),
                            emit_expr(&args[0])
                        ));
                    }
                }
            }
            None
        }
        crate::expr::IrHint::StringBuilderResult => {
            if let ExprNode::Var { name, .. } = &*e.node {
                return Some(camel(name.as_str()));
            }
            None
        }
    }
}

/// Render a top-level runtime constant's value. Non-empty hash/array
/// literals drop the `as [String: Any?]` pin so Swift infers the
/// homogeneous element type (`STATUS_CODES`-style tables become
/// `[String: Int]`, not `[String: Any?]`).
pub fn emit_constant_for_runtime(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Hash { entries, .. } if !entries.is_empty() => {
            let pairs: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("[{}]", pairs.join(", "))
        }
        ExprNode::Array { elements, .. } if !elements.is_empty() => {
            let els: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", els.join(", "))
        }
        _ => emit_expr(e),
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_empty_branch(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// The branch's single value-expression, when it is one (possibly
/// wrapped in a one-element Seq) — the shape a ternary can carry.
fn single_value_expr(e: &Expr) -> Option<&Expr> {
    match &*e.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => single_value_expr(&exprs[0]),
        ExprNode::Seq { .. }
        | ExprNode::If { .. }
        | ExprNode::While { .. }
        | ExprNode::Case { .. }
        | ExprNode::Assign { .. }
        | ExprNode::OpAssign { .. }
        | ExprNode::Return { .. }
        | ExprNode::Raise { .. }
        | ExprNode::Super { .. }
        | ExprNode::Next { .. }
        | ExprNode::Break { .. } => None,
        // Assignment in Send spelling (`x[k] = v`, `recv.foo = v`) is a
        // statement, not a ternary-carriable value — and so is an
        // iteration (`each` returns its receiver in Ruby, typing the If
        // as a value, but the Swift forEach is Void).
        ExprNode::Send { method, block, .. }
            if block.is_some()
                || method.as_str() == "[]="
                || (method.as_str().ends_with('=')
                    && !matches!(method.as_str(), "==" | "!=" | "<=" | ">=")) =>
        {
            None
        }
        _ => Some(e),
    }
}

fn emit_node(n: &ExprNode, e: &Expr) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => {
            let n = camel(name.as_str());
            if NONNULL_PROPS.with(|s| s.borrow().contains(&n)) {
                format!("{n}!")
            } else {
                n
            }
        }
        // Instance variable → property reference; force-unwrapped when
        // the enclosing branch's nil-guard proved it non-nil.
        ExprNode::Ivar { name } => {
            let n = camel(name.as_str());
            if NONNULL_PROPS.with(|s| s.borrow().contains(&n)) {
                format!("{n}!")
            } else {
                n
            }
        }
        ExprNode::SelfRef => "self".to_string(),
        ExprNode::Const { path } => {
            let joined: Vec<String> = path.iter().map(|s| s.to_string()).collect();
            super::naming::type_name(&joined.join("::"))
        }
        ExprNode::Hash { entries, .. } => emit_hash(entries),
        ExprNode::Array { elements, .. } => emit_array(elements, e),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, left, right, .. } => emit_bool_op(*op, left, right, e),
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args, block.as_ref())
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // A value-typed If with simple branches is Ruby's ternary in
            // value position (arg slots, interpolations) — Swift's `if`
            // is a statement there (unlike Kotlin), so emit `c ? a : b`.
            if matches!(e.ty.as_ref(), Some(t) if !matches!(t, crate::ty::Ty::Nil))
                && !is_empty_branch(else_branch)
            {
                if let (Some(t), Some(f)) =
                    (single_value_expr(then_branch), single_value_expr(else_branch))
                {
                    return format!(
                        "({} ? {} : {})",
                        emit_expr(cond),
                        emit_expr(t),
                        emit_expr(f)
                    );
                }
            }
            emit_if(cond, then_branch, else_branch)
        }
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms, false),
        ExprNode::Seq { exprs } => emit_stmts(exprs, false),
        ExprNode::Assign { target, value } => emit_assign(target, value),
        ExprNode::OpAssign { target, op, value } => emit_op_assign(target, *op, value),
        ExprNode::Return { value } => {
            let returns_value = RETURNS_VALUE.with(|r| *r.borrow());
            // `return nil` is a bare `return` only in a Void method; a
            // value-returning (Optional) method needs the literal.
            if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                if returns_value {
                    "return nil".to_string()
                } else {
                    "return".to_string()
                }
            } else if !returns_value {
                // Ruby's `return self`-style value in a Void method:
                // Swift rejects non-void returns. Pure values drop;
                // side-effecting expressions run first.
                if matches!(
                    &*value.node,
                    ExprNode::SelfRef
                        | ExprNode::Var { .. }
                        | ExprNode::Ivar { .. }
                        | ExprNode::Lit { .. }
                ) {
                    "return".to_string()
                } else {
                    format!("{}\nreturn", emit_expr(value))
                }
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::While { cond, body, until_form } => {
            let c = emit_expr(cond);
            let c = if *until_form { format!("!({c})") } else { c };
            format!("while {c} {{\n{}\n}}", indent(&emit_expr(body)))
        }
        ExprNode::Raise { value } => emit_raise(value),
        // `super(msg)` inside an error class's `initialize` assigns the
        // synthesized message property (`Error` is a protocol — there is
        // no super-initializer to delegate to). Elsewhere it stays a
        // placeholder until real designated-init delegation is needed.
        ExprNode::Super { args } => {
            if IN_ERROR_CLASS.with(|f| *f.borrow()) {
                match args.as_ref().and_then(|a| a.first()) {
                    Some(msg) => format!("self.message = {}", emit_expr(msg)),
                    None => "self.message = \"\"".to_string(),
                }
            } else if INIT_SUPER.with(|f| *f.borrow()) {
                let rendered: Vec<String> = args
                    .as_ref()
                    .map(|a| a.iter().map(emit_expr).collect())
                    .unwrap_or_default();
                format!("super.init({})", rendered.join(", "))
            } else {
                "/* super() */".to_string()
            }
        }
        ExprNode::Cast { value, target_ty } => emit_cast(value, target_ty),
        ExprNode::Lambda { params, body, .. } => emit_lambda(params, body),
        // No throwing yet (Phase 3 `throws` pass), so the rescue-modifier
        // fallback shape degrades to just the expression — visible TODO.
        ExprNode::RescueModifier { expr, fallback } => format!(
            "{} /* TODO rescue-modifier fallback: {} */",
            emit_expr(expr),
            emit_expr(fallback)
        ),
        other => format!("/* TODO {} */", other.kind_str()),
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        // Plain int literal: Swift adapts the literal to the expected
        // type, and `Int` is already 64-bit — no suffix dance.
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                format!("{value}")
            }
        }
        Literal::Str { value } => format!("\"{}\"", escape_str(value)),
        // No symbol type in Swift → string.
        Literal::Sym { value } => format!("\"{}\"", escape_str(value.as_str())),
        Literal::Regex { pattern, .. } => {
            format!("try! NSRegularExpression(pattern: \"{}\")", escape_str(pattern))
        }
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Swift has no \b/\f escapes and rejects raw control bytes
            // in source — render them (and any other control char) as
            // the universal \u{XX} escape.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

fn emit_hash(entries: &[(Expr, Expr)]) -> String {
    if entries.is_empty() {
        return "[String: Any?]()".to_string();
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
        .collect();
    // Heterogeneous values defeat dictionary-literal inference, so pin
    // the element type the way Kotlin pins `mutableMapOf<String, Any?>`.
    format!("([{}] as [String: Any?])", pairs.join(", "))
}

fn emit_array(elements: &[Expr], e: &Expr) -> String {
    if elements.is_empty() {
        // Use the annotated element type when present, else Any?.
        if let Some(crate::ty::Ty::Array { elem }) = e.ty.as_ref() {
            return format!("[{}]()", swift_ty(elem));
        }
        return "[Any?]()".to_string();
    }
    let els: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("[{}]", els.join(", "))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("\"");
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(&escape_str(value)),
            InterpPart::Expr { expr } => {
                out.push_str(&format!("\\({})", emit_expr(expr)));
            }
        }
    }
    out.push('"');
    out
}

fn emit_bool_op(op: BoolOpKind, left: &Expr, right: &Expr, e: &Expr) -> String {
    let l = emit_expr(left);
    let r = emit_expr(right);
    match op {
        BoolOpKind::And => format!("{l} && {r}"),
        // `||` is logical-or for Bool results, but Ruby's `x || default`
        // nil-coalescing idiom maps to Swift's `??` when the result
        // isn't a Bool.
        BoolOpKind::Or => {
            if matches!(e.ty.as_ref(), Some(crate::ty::Ty::Bool)) {
                format!("{l} || {r}")
            } else {
                format!("({l} ?? {r})")
            }
        }
    }
}

fn emit_if(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    // `if !x.nil? && rest(x)` — the REST of the condition itself needs
    // the binding, so it rewrites to Swift's conjunctive optional
    // binding: `if let x = x, rest { … }` (the rebound `x` is non-opt
    // in both the rest-condition and the then-branch).
    if let Some(s) = try_iflet_conjunction(cond, then_branch, else_branch) {
        return s;
    }
    // `if x.is_a?(T)` narrows via shadow-rebinding: `if let x = x as? T`.
    // Swift's `is` does NOT smart-cast (unlike Kotlin), so the branch
    // body's uses of `x` at type T only compile with the rebind.
    let c = match isa_narrow_cond(cond) {
        Some(narrowed) => narrowed,
        None => emit_expr(cond),
    };
    // Optional-property narrowing: `!x.nil?` proves x non-nil in the
    // then-branch; `x.nil?` proves it in the else-branch. Reads inside
    // the proven branch force-unwrap.
    let then_nonnull = props_proven_nonnull(cond);
    let else_nonnull = prop_nil_checked(cond).into_iter().collect::<Vec<_>>();
    let then_empty = is_empty_branch(then_branch);
    let else_empty = is_empty_branch(else_branch);
    // An empty then-branch with a real else (the lowered guard shape
    // `if c then nil else X`) inverts — Swift rejects a bare `nil`
    // statement. (Not reachable for the narrowing cond shape: a `nil?`
    // cond with a real else fuses to if-let upstream.)
    if then_empty && !else_empty {
        let els = indent(&with_nonnull(&else_nonnull, else_branch));
        return format!("if !({c}) {{\n{els}\n}}");
    }
    let then = indent(&with_nonnull(&then_nonnull, then_branch));
    if else_empty {
        format!("if {c} {{\n{then}\n}}")
    } else {
        let els = indent(&with_nonnull(&else_nonnull, else_branch));
        format!("if {c} {{\n{then}\n}} else {{\n{els}\n}}")
    }
}

/// `if !x.nil? && rest { then }` → `if let x = x, rest { then }`.
/// Fires only when the leading conjunct is a negated nil-check over an
/// Optional-typed simple read; `rest` and the then-branch emit with the
/// rebound (non-optional) name — NO force-unwrap inside.
fn try_iflet_conjunction(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> Option<String> {
    // Unwrap a `!` (either IR spelling) to its operand.
    fn negated(e: &Expr) -> Option<&Expr> {
        match &*e.node {
            ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "!" && args.is_empty() =>
            {
                Some(r)
            }
            ExprNode::Send { recv: None, method, args, .. }
                if method.as_str() == "!" && args.len() == 1 =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }
    // Two source spellings of "present":
    //   !x.nil? && rest            → if let x = x, rest
    //   !(x.nil? || more)          → if let x = x, !(more)
    let (n, rest) = match &*cond.node {
        ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } => {
            let inner = negated(left)?;
            (prop_nil_checked(inner)?, emit_expr(right))
        }
        _ => {
            let inner = negated(cond)?;
            let ExprNode::BoolOp { op: BoolOpKind::Or, left, right, .. } = &*inner.node else {
                return None;
            };
            (prop_nil_checked(left)?, format!("!({})", emit_expr(right)))
        }
    };
    let then = indent(&emit_expr(then_branch));
    if is_empty_branch(else_branch) {
        Some(format!("if let {n} = {n}, {rest} {{\n{then}\n}}"))
    } else {
        let els = indent(&emit_expr(else_branch));
        Some(format!("if let {n} = {n}, {rest} {{\n{then}\n}} else {{\n{els}\n}}"))
    }
}

/// Emit a branch with extra proven-non-nil props in scope.
fn with_nonnull(props: &[String], branch: &Expr) -> String {
    let added: Vec<String> = NONNULL_PROPS.with(|s| {
        let mut set = s.borrow_mut();
        props.iter().filter(|p| set.insert((*p).clone())).cloned().collect()
    });
    let out = emit_expr(branch);
    NONNULL_PROPS.with(|s| {
        let mut set = s.borrow_mut();
        for p in &added {
            set.remove(p);
        }
    });
    out
}

/// The binding a `nil?` receiver reads, when it IS a simple read — an
/// ivar, a zero-arg self-send, a local/param Var, or the view lowerer's
/// bare-Send param spelling.
fn prop_read_name(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Ivar { name } => Some(camel(name.as_str())),
        ExprNode::Var { name, .. } => Some(camel(name.as_str())),
        ExprNode::Send { recv: Some(r), method, args, .. }
            if args.is_empty() && matches!(&*r.node, ExprNode::SelfRef) =>
        {
            Some(camel(method.as_str()))
        }
        ExprNode::Send { recv: None, method, args, .. }
            if args.is_empty() && is_param(method.as_str()) =>
        {
            Some(camel(method.as_str()))
        }
        _ => None,
    }
}

/// Props proven non-nil when `cond` is true: `!x.nil?` (both `!` IR
/// spellings) and `&&`-conjunctions thereof.
fn props_proven_nonnull(cond: &Expr) -> Vec<String> {
    match &*cond.node {
        ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } => {
            let mut v = props_proven_nonnull(left);
            v.extend(props_proven_nonnull(right));
            v
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            prop_nil_checked(r).into_iter().collect()
        }
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "!" && args.len() == 1 =>
        {
            prop_nil_checked(&args[0]).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// The prop a bare `x.nil?` cond checks (proven non-nil in the ELSE
/// branch). Only Optional-typed reads participate — force-unwrapping a
/// non-optional is a compile error, and a nil-check on one is just a
/// tautology the analyzer left in.
fn prop_nil_checked(cond: &Expr) -> Option<String> {
    match &*cond.node {
        // Both nil-check spellings: `x.nil?` and `x == nil`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if (method.as_str() == "nil?" && args.is_empty())
                || (method.as_str() == "=="
                    && args.len() == 1
                    && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Nil })) =>
        {
            let is_opt = |t: &crate::ty::Ty| {
                matches!(t, crate::ty::Ty::Union { variants }
                    if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil)))
            };
            let optionalish = r.ty.as_ref().map_or(false, is_opt)
                // A Var read (or bare-Send param read) with no stamped
                // ty: the method signature's param type is the authority.
                || match &*r.node {
                    ExprNode::Var { name, .. } => {
                        let n = camel(name.as_str());
                        PARAM_TYPES.with(|p| p.borrow().get(&n).map_or(false, is_opt))
                    }
                    ExprNode::Send { recv: None, method, args, .. }
                        if args.is_empty() && is_param(method.as_str()) =>
                    {
                        let n = camel(method.as_str());
                        PARAM_TYPES.with(|p| p.borrow().get(&n).map_or(false, is_opt))
                    }
                    _ => false,
                };
            if optionalish {
                prop_read_name(r)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Ruby classes with a direct Swift `as?` target in the runtime's value
/// world.
fn isa_swift_type(class_name: &str) -> Option<&'static str> {
    Some(match class_name {
        "Integer" => "Int",
        "Float" => "Double",
        "String" | "Symbol" => "String",
        "Hash" => "[String: Any?]",
        "Array" => "[Any?]",
        _ => return None,
    })
}

/// `x.is_a?(T)` as an if-condition over a Var → `let x = x as? T`.
fn isa_narrow_cond(cond: &Expr) -> Option<String> {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "is_a?" && method.as_str() != "kind_of?" {
        return None;
    }
    let ExprNode::Var { name, .. } = &*r.node else {
        return None;
    };
    let [arg] = args.as_slice() else {
        return None;
    };
    let ExprNode::Const { path } = &*arg.node else {
        return None;
    };
    let cls = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
    let swift = isa_swift_type(&cls)?;
    let n = camel(name.as_str());
    Some(format!("let {n} = {n} as? {swift}"))
}

/// `case` → `switch`. Swift `switch` is a statement, not an expression
/// (unlike Kotlin `when`), so a `Case` in return position is rendered
/// with `returning: true`: each arm body gets the `return` pushed into
/// it (via `wrap_return`), and a missing default returns `nil` (Ruby's
/// `case` without `else` evaluates to nil).
fn emit_case(scrutinee: &Expr, arms: &[Arm], returning: bool) -> String {
    let s = emit_expr(scrutinee);
    let render = |e: &Expr| if returning { wrap_return(e) } else { emit_expr(e) };
    let mut lines = Vec::new();
    let mut has_default = false;
    for arm in arms {
        let body = indent(&render(&arm.body));
        match &arm.pattern {
            Pattern::Wildcard | Pattern::Bind { .. } => {
                has_default = true;
                lines.push(format!("default:\n{body}"));
            }
            Pattern::Lit { value } => {
                lines.push(format!("case {}:\n{body}", emit_literal(value)));
            }
            other => {
                lines.push(format!("/* TODO pattern {other:?} */ default:\n{body}"));
                has_default = true;
            }
        }
    }
    if !has_default {
        let fallback = if returning { "default:\n    return nil" } else { "default:\n    break" };
        lines.push(fallback.to_string());
    }
    format!("switch {s} {{\n{}\n}}", lines.join("\n"))
}

/// Emit a statement sequence. Fuses the `x = <optional>; if x.nil?
/// return` pair into Swift's `guard let x = <optional> else { return }` —
/// Swift has NO flow-narrowing (unlike Kotlin's smart casts), so without
/// the fusion every later read of `x` would fail to compile against its
/// optional type. With `returning: true` the final statement gets
/// `wrap_return`.
pub(super) fn emit_stmts(exprs: &[Expr], returning: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut i = 0;
    while i < exprs.len() {
        let is_last = i == exprs.len() - 1;
        // A bare `nil` statement (a lowered no-op branch filler) has no
        // contextual type in Swift — drop it.
        if !(returning && is_last)
            && matches!(&*exprs[i].node, ExprNode::Lit { value: Literal::Nil })
        {
            i += 1;
            continue;
        }
        // guard-let fusion: Assign(Var x, v) followed by
        // `if x.nil? { <terminal> }` (empty else).
        if i + 1 < exprs.len() {
            if let Some(fused) = try_guard_let(&exprs[i], &exprs[i + 1]) {
                lines.push(fused);
                i += 2;
                continue;
            }
        }
        // Standalone optional-param nil-guard → `guard let x = x`.
        if !(returning && is_last) {
            if let Some(guard) = try_param_guard(&exprs[i]) {
                lines.push(guard);
                i += 1;
                continue;
            }
        }
        if returning && is_last {
            lines.push(wrap_return(&exprs[i]));
        } else {
            lines.push(emit_expr(&exprs[i]));
        }
        i += 1;
    }
    lines.join("\n")
}

fn try_guard_let(assign: &Expr, guard: &Expr) -> Option<String> {
    let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*assign.node else {
        return None;
    };
    let n = camel(name.as_str());
    // A reassigned local can't become a binding constant.
    if REASSIGNED.with(|r| r.borrow().contains(&n)) {
        return None;
    }
    let ExprNode::If { cond, then_branch, else_branch } = &*guard.node else {
        return None;
    };
    // cond must be `x.nil?` (either IR spelling).
    let nil_check = match &*cond.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "nil?" && args.is_empty() =>
        {
            matches!(&*r.node, ExprNode::Var { name: vn, .. } if camel(vn.as_str()) == n)
        }
        _ => false,
    };
    if !nil_check {
        return None;
    }
    // Three shapes:
    //   nil-branch terminal, no else      → guard let x = v else { … }
    //   nil-branch empty, else present    → if let x = v { else-branch }
    //   both present                      → if let x = v { else } else { then }
    let then_empty = is_empty_branch(then_branch);
    let else_empty = is_empty_branch(else_branch);
    if else_empty && branch_is_terminal(then_branch) {
        DECLARED.with(|d| {
            d.borrow_mut().insert(n.clone());
        });
        let body = emit_expr(then_branch);
        return Some(format!(
            "guard let {n} = {} else {{\n{}\n}}",
            emit_expr(value),
            indent(&body)
        ));
    }
    if !else_empty {
        DECLARED.with(|d| {
            d.borrow_mut().insert(n.clone());
        });
        let val = emit_expr(value);
        let some_body = indent(&emit_expr(else_branch));
        if then_empty {
            return Some(format!("if let {n} = {val} {{\n{some_body}\n}}"));
        }
        let nil_body = indent(&emit_expr(then_branch));
        return Some(format!(
            "if let {n} = {val} {{\n{some_body}\n}} else {{\n{nil_body}\n}}"
        ));
    }
    None
}

/// Ruby string slice with a Range: `str[b..]` → dropFirst, `str[..e]` →
/// prefix (inclusive end keeps e+1 chars), both-ended → the combination.
fn emit_slice_range(
    rs: &str,
    begin: Option<&Expr>,
    end: Option<&Expr>,
    exclusive: bool,
) -> String {
    match (begin, end) {
        (Some(b), None) => format!("String({rs}.dropFirst({}))", emit_expr(b)),
        (None, Some(e)) => {
            let e_s = emit_expr(e);
            if exclusive {
                format!("String({rs}.prefix({e_s}))")
            } else {
                format!("String({rs}.prefix(({e_s}) + 1))")
            }
        }
        (Some(b), Some(e)) => {
            let b_s = emit_expr(b);
            let e_s = emit_expr(e);
            let len = if exclusive {
                format!("({e_s}) - ({b_s})")
            } else {
                format!("({e_s}) - ({b_s}) + 1")
            };
            format!("String({rs}.dropFirst({b_s}).prefix({len}))")
        }
        (None, None) => format!("String({rs})"),
    }
}

/// A standalone `if x.nil? { <terminal> }` over an Optional-typed
/// binding rewrites to `guard let x = x else { … }`, shadow-rebinding
/// the name non-optional for the rest of the scope — Swift's spelling
/// of the narrowing Kotlin gets from smart casts. The compound form
/// `if x.nil? || <more(x)> { <terminal> }` becomes
/// `guard let x = x, !(<more>) else { … }` — `x` inside `<more>` reads
/// the unwrapped binding.
fn try_param_guard(stmt: &Expr) -> Option<String> {
    let ExprNode::If { cond, then_branch, else_branch } = &*stmt.node else {
        return None;
    };
    if !is_empty_branch(else_branch) || !branch_is_terminal(then_branch) {
        return None;
    }
    // Split `x.nil?` vs `x.nil? || rest`.
    let (nil_check, rest) = match &*cond.node {
        ExprNode::BoolOp { op: BoolOpKind::Or, left, right, .. } => (left, Some(right)),
        _ => (cond, None),
    };
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*nil_check.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let ExprNode::Var { name, .. } = &*r.node else {
        return None;
    };
    // Only when the binding is provably Optional — `guard let` over a
    // non-optional is a compile error, while the plain `if` is merely a
    // tautology warning.
    let optionalish = matches!(
        r.ty.as_ref(),
        Some(crate::ty::Ty::Union { variants })
            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    );
    if !optionalish {
        return None;
    }
    let n = camel(name.as_str());
    let extra = match rest {
        Some(rhs) => format!(", !({})", emit_expr(rhs)),
        None => String::new(),
    };
    Some(format!(
        "guard let {n} = {n}{extra} else {{\n{}\n}}",
        indent(&emit_expr(then_branch))
    ))
}

fn branch_is_terminal(e: &Expr) -> bool {
    if is_raise_expr(e) {
        return true;
    }
    match &*e.node {
        ExprNode::Return { .. } | ExprNode::Raise { .. } | ExprNode::Break { .. }
        | ExprNode::Next { .. } => true,
        ExprNode::Seq { exprs } => exprs.last().map_or(false, branch_is_terminal),
        _ => false,
    }
}

/// Prefix `return` for a body in return position — recursing into the
/// shapes where the `return` must land deeper: a `Seq`'s final statement,
/// every arm of a `Case` (Swift `switch` is not an expression), and both
/// branches of a two-armed `If` (multi-statement Swift `if` branches
/// aren't expressions either). Terminal/valueless statements pass
/// through unprefixed.
pub(super) fn wrap_return(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => emit_stmts(exprs, true),
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms, true),
        ExprNode::If { cond, then_branch, else_branch } if !is_empty_branch(else_branch) => {
            let c = emit_expr(cond);
            format!(
                "if {c} {{\n{}\n}} else {{\n{}\n}}",
                indent(&wrap_return(then_branch)),
                indent(&wrap_return(else_branch))
            )
        }
        ExprNode::Return { .. }
        | ExprNode::Raise { .. }
        | ExprNode::While { .. }
        | ExprNode::Assign { .. }
        | ExprNode::OpAssign { .. }
        | ExprNode::Super { .. }
        | ExprNode::Next { .. }
        | ExprNode::Break { .. } => emit_expr(e),
        // A raise in Send spelling (throw/fatalError) is terminal — no
        // `return` prefix (fatalError's Never satisfies the return path).
        _ if is_raise_expr(e) => emit_expr(e),
        _ => format!("return {}", emit_expr(e)),
    }
}

/// An assignment's value: an empty container literal leans on the
/// (already-typed) target — `[:]` / `[]` — instead of pinning
/// `[String: Any?]()`.
fn assign_value(value: &Expr) -> String {
    match &*value.node {
        ExprNode::Hash { entries, .. } if entries.is_empty() => "[:]".to_string(),
        ExprNode::Array { elements, .. } if elements.is_empty() => "[]".to_string(),
        _ => emit_expr(value),
    }
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let val = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let n = camel(name.as_str());
            let already = DECLARED.with(|d| d.borrow().contains(&n));
            if already {
                format!("{n} = {}", assign_value(value))
            } else {
                let is_var = REASSIGNED.with(|r| r.borrow().contains(&n));
                DECLARED.with(|d| {
                    d.borrow_mut().insert(n.clone());
                });
                let kw = if is_var { "var" } else { "let" };
                // `var x = nil` has no contextual type in Swift (hard
                // error, not just bad inference like Kotlin's `Nothing?`);
                // annotate from a later non-nil assignment when we have
                // one.
                let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
                if is_nil {
                    if let Some(ty) = NIL_TYPES.with(|t| t.borrow().get(&n).cloned()) {
                        return format!("{kw} {n}: {ty} = {val}");
                    }
                    return format!("{kw} {n}: Any? = {val}");
                }
                // An empty-container initializer takes its declared type
                // from the population scan (`params = {}` later written
                // string→string becomes `var params: [String: String]`).
                let is_empty_container = matches!(
                    &*value.node,
                    ExprNode::Hash { entries, .. } if entries.is_empty()
                ) || matches!(
                    &*value.node,
                    ExprNode::Array { elements, .. } if elements.is_empty()
                );
                if is_empty_container {
                    if let Some(ct) = CONTAINER_TYPES.with(|t| t.borrow().get(&n).cloned()) {
                        let lit = if ct.contains(':') { "[:]" } else { "[]" };
                        return format!("{kw} {n}: {ct} = {lit}");
                    }
                }
                format!("{kw} {n} = {val}")
            }
        }
        // `self.`-qualified so constructor params can shadow properties
        // (`init(_ verb: String)` assigning the `verb` property) — but
        // bare in module enums (static funcs have no `self`).
        LValue::Ivar { name } => {
            let val = coerce_for_prop(name.as_str(), value, assign_value(value));
            if IN_MODULE.with(|f| *f.borrow()) {
                format!("{} = {val}", camel(name.as_str()))
            } else {
                format!("self.{} = {val}", camel(name.as_str()))
            }
        }
        LValue::Attr { recv, name } => {
            let val = coerce_for_prop_assign(recv, name.as_str(), value, assign_value(value));
            format!("{}.{} = {val}", emit_expr(recv), camel(name.as_str()))
        }
        LValue::Index { recv, index } => {
            format!("{}[{}] = {val}", emit_expr(recv), emit_expr(index))
        }
        LValue::Const { path } => {
            let p = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".");
            format!("{p} = {val}")
        }
    }
}

/// The plan's throws split (delta 1): a raise of an Error-conforming
/// class is a real `throw` (control flow: RecordNotFound → 404,
/// RecordInvalid); everything else — message-only raises,
/// NotImplementedError — is a "never happens" `fatalError`, keeping the
/// `throws` ripple confined to the genuinely-throwing surface.
fn emit_raise(value: &Expr) -> String {
    match &*value.node {
        ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => {
            format!("fatalError({})", emit_expr(value))
        }
        // `raise RecordNotFound` / `raise RecordNotFound.new(...)`.
        ExprNode::Const { path } => {
            let joined = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
            let cls = super::naming::type_name(&joined);
            if is_error_class_name(&cls) {
                format!("throw {cls}()")
            } else {
                format!("fatalError(\"{joined}\")")
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "new" => {
            if let ExprNode::Const { path } = &*r.node {
                let joined = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
                let cls = super::naming::type_name(&joined);
                if is_error_class_name(&cls) {
                    return format!("throw {}", emit_expr(value));
                }
            }
            format!("fatalError(\"\\(String(describing: {}))\")", emit_expr(value))
        }
        _ => format!("fatalError(\"\\(String(describing: {}))\")", emit_expr(value)),
    }
}

/// A raise in either IR spelling — terminal for return-wrapping.
pub(super) fn is_raise_expr(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Raise { .. } => true,
        ExprNode::Send { recv: None, method, args, .. } => {
            method.as_str() == "raise" && !args.is_empty()
        }
        _ => false,
    }
}

/// Does this body contain a raise that the classification turns into a
/// real `throw`? (Drives the `throws` marking on method signatures.)
pub(super) fn body_throws(e: &Expr) -> bool {
    let direct = match &*e.node {
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "raise" => {
            args.first().map_or(false, |a| {
                matches!(&*a.node, ExprNode::Const { path }
                    if is_error_class_name(&super::naming::type_name(
                        &path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"))))
            })
        }
        ExprNode::Raise { value } => match &*value.node {
            ExprNode::Const { path } => is_error_class_name(&super::naming::type_name(
                &path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"),
            )),
            ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "new" => {
                matches!(&*r.node, ExprNode::Const { path }
                    if is_error_class_name(&super::naming::type_name(
                        &path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"))))
            }
            _ => false,
        },
        _ => false,
    };
    direct || children(e).into_iter().any(body_throws)
}

/// The lowerer inserts `Cast` at untyped-row boundaries to mean "coerce
/// to this column type". Plan delta 6: in the row paths the box already
/// holds the target type (sqlite column values), so the right Swift
/// spelling is the `as!` downcast — Swift's dynamic cast sees through
/// nested optionals, which covers the `[String: Any?]` double-optional
/// lookup. (The genuinely-converting string→number case — `from_params`
/// input — arrives with the controller layer and gets `Int("\(v)")!`
/// when a consumer forces it.)
fn emit_cast(value: &Expr, target_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let v = emit_expr(value);
    match target_ty {
        Ty::Int => format!("({v} as! Int)"),
        Ty::Float => format!("({v} as! Double)"),
        Ty::Str | Ty::Sym => format!("({v} as! String)"),
        _ => format!("({v} as! {})", swift_ty(target_ty)),
    }
}

fn emit_lambda(params: &[crate::ident::Symbol], body: &Expr) -> String {
    let body_s = emit_expr(body);
    if params.is_empty() {
        format!("{{ {body_s} }}")
    } else {
        // Parenthesized param list: required for the `(k, v)` tuple
        // destructure Dictionary.forEach needs, harmless elsewhere.
        let ps: Vec<String> = params.iter().map(|p| camel(p.as_str())).collect();
        format!("{{ ({}) in {body_s} }}", ps.join(", "))
    }
}

/// Methods that look like 0-arg attribute reads but are real method calls
/// (need `()` in Swift). Everything else with a receiver and no args is
/// emitted as property access.
fn forces_parens(method: &str) -> bool {
    matches!(
        method,
        "save" | "save!" | "destroy" | "destroy!" | "reload" | "validate" | "dup" | "clone"
    )
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();

    // Negation — `!x` arrives BOTH prefix (`Send{None, "!", [x]}`) and
    // postfix (`Send{Some(x), "!", []}`), the same two IR shapes the
    // Kotlin emitter reconciles.
    if method == "!" {
        if let (Some(r), 0) = (recv, args.len()) {
            return format!("!({})", emit_expr(r));
        }
        if let (None, 1) = (recv, args.len()) {
            return format!("!({})", args_s[0]);
        }
    }

    // Bareword `raise Class, msg` / `raise msg` (the Send spelling; the
    // Raise node is the other). The plan's throws split: an
    // Error-conforming class throws; everything else is a "never
    // happens" fatalError.
    if method == "raise" && recv.is_none() && !args.is_empty() {
        if let ExprNode::Const { path } = &*args[0].node {
            let joined = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
            let cls = super::naming::type_name(&joined);
            if is_error_class_name(&cls) {
                let rest = args_s[1..].join(", ");
                return format!("throw {cls}({rest})");
            }
            // NotImplementedError and friends.
            let msg = args_s
                .get(1)
                .cloned()
                .unwrap_or_else(|| format!("\"{joined}\""));
            return format!("fatalError({msg})");
        }
        return format!("fatalError({})", args_s[0]);
    }

    // Constructor: `X.new(...)` → `X(...)`. Implicit-self `new(attrs)`
    // in a class method → `Self(...)` (dynamic, so `Article.create`
    // builds an Article; requires `required init`, which the init emit
    // marks).
    if method == "new" {
        if let Some(r) = recv {
            return format!("{}({})", emit_expr(r), args_s.join(", "));
        }
        return format!("Self({})", args_s.join(", "));
    }

    // `self.class.X(...)` → `Self.X(...)` — Swift statics are NOT
    // reachable by bare name from instance methods (unlike Kotlin
    // companions), and `Self` keeps the dispatch dynamic so per-model
    // overrides resolve.
    if let Some(r) = recv {
        if let ExprNode::Send { recv: Some(inner), method: m2, args: a2, .. } = &*r.node {
            if m2.as_str() == "class"
                && a2.is_empty()
                && matches!(&*inner.node, ExprNode::SelfRef)
            {
                return format!("Self.{}({})", camel(method), args_s.join(", "));
            }
        }
    }

    // Attribute setter: `recv.foo = v` arrives as a Send named `foo=`.
    if let (Some(r), 1) = (recv, args.len()) {
        if method.ends_with('=') && !matches!(method, "==" | "!=" | "<=" | ">=") {
            let base = &method[..method.len() - 1];
            let val = coerce_for_prop_assign(r, base, &args[0], args_s[0].clone());
            return format!("{}.{} = {}", emit_expr(r), camel(base), val);
        }
    }

    // `is_a?` outside an if-condition (no narrowing needed): TrueClass/
    // FalseClass become Bool-value tests, mapped classes an `as?`-test.
    if (method == "is_a?" || method == "kind_of?") && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            let rs = emit_expr(r);
            let cls = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
            return match cls.as_str() {
                "TrueClass" => format!("({rs} as? Bool) == true"),
                "FalseClass" => format!("({rs} as? Bool) == false"),
                _ => match isa_swift_type(&cls) {
                    Some(t) => format!("({rs} as? {t}) != nil"),
                    None => format!("({rs} is {})", super::naming::type_name(&cls)),
                },
            };
        }
    }

    // `gsub(regex, map)` — regex replace with a lookup table; no clean
    // inline Swift idiom (NSRegularExpression is verbose, native Regex
    // closures are generic-fiddly), so it dispatches to the hand-written
    // RhString primitive. The two-string form is a plain
    // replacingOccurrences.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            let rs = emit_expr(r);
            if matches!(&*args[1].node, ExprNode::Hash { .. })
                || matches!(args[1].ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
            {
                return format!("RhString.gsubMap({rs}, {}, {})", args_s[0], args_s[1]);
            }
            if matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { .. } }) {
                return format!(
                    "{rs}.replacingOccurrences(of: {}, with: {})",
                    args_s[0], args_s[1]
                );
            }
            return format!("RhString.gsub({rs}, {}, {})", args_s[0], args_s[1]);
        }
    }

    // Binary operators with a receiver and one arg.
    if let (Some(r), 1) = (recv, args.len()) {
        if matches!(
            method,
            "+" | "-" | "*" | "/" | "%" | "<" | ">" | "<=" | ">=" | "==" | "!=" | "&&" | "||"
        ) {
            return format!("{} {} {}", emit_expr(r), method, args_s[0]);
        }
        // `<<` / `push` → Array.append.
        if method == "<<" || method == "push" {
            return format!("{}.append({})", emit_expr(r), args_s[0]);
        }
        // Index read `recv[k]` — or a Range arg, the Ruby string-slice
        // `str[b..]` / `str[..e]` (Swift's String index API has no
        // integer subscripts; dropFirst/prefix is the idiom).
        if method == "[]" {
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                return emit_slice_range(
                    &emit_expr(r),
                    begin.as_ref(),
                    end.as_ref(),
                    *exclusive,
                );
            }
            // Ruby's nil-on-empty `records[-1]` is Swift's Optional
            // `.last`.
            if matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { value: -1 } }) {
                return format!("{}.last", emit_expr(r));
            }
            return format!("{}[{}]", emit_expr(r), args_s[0]);
        }
        // Hash key test (Swift dictionaries have no containsKey; the
        // index-vs-nil test is the idiom).
        if method == "key?" || method == "has_key?" {
            return format!("({}[{}] != nil)", emit_expr(r), args_s[0]);
        }
        // String split — components(separatedBy:) keeps Ruby's leading
        // empty field ("/a".split("/") → ["", "a"]), which
        // split(separator:) would drop.
        if method == "split" {
            return format!("{}.components(separatedBy: {})", emit_expr(r), args_s[0]);
        }
        if method == "start_with?" {
            return format!("{}.hasPrefix({})", emit_expr(r), args_s[0]);
        }
        if method == "end_with?" {
            return format!("{}.hasSuffix({})", emit_expr(r), args_s[0]);
        }
        if method == "include?" {
            return format!("{}.contains({})", emit_expr(r), args_s[0]);
        }
        if method == "join" {
            return format!("{}.joined(separator: {})", emit_expr(r), args_s[0]);
        }
        // Dictionary shims (Ruby Hash surface → Swift Dictionary).
        if method == "delete" && recv_is_hash(r) {
            return format!("{}.removeValue(forKey: {})", emit_expr(r), args_s[0]);
        }
        if method == "merge" {
            return format!(
                "{}.merging({}) {{ (_, new) in new }}",
                emit_expr(r),
                args_s[0]
            );
        }
    }
    if let (Some(r), 2) = (recv, args.len()) {
        // `fetch(k, default)` → nil-coalesced index.
        if method == "fetch" {
            return format!("({}[{}] ?? {})", emit_expr(r), args_s[0], args_s[1]);
        }
        // `tr(from, to)` — the runtime's single-char uses map to plain
        // replacement.
        if method == "tr" {
            return format!(
                "{}.replacingOccurrences(of: {}, with: {})",
                emit_expr(r),
                args_s[0],
                args_s[1]
            );
        }
    }
    if let (Some(r), 2) = (recv, args.len()) {
        if method == "[]=" {
            return format!("{}[{}] = {}", emit_expr(r), args_s[0], args_s[1]);
        }
        // Ruby `str[start, len]` positional slice.
        if method == "[]" {
            return format!(
                "String({}.dropFirst({}).prefix({}))",
                emit_expr(r),
                args_s[0],
                args_s[1]
            );
        }
    }

    // Zero-arg receiver sends: builtin coercions, then property vs method.
    if let (Some(r), true) = (recv, args.is_empty() && block.is_none()) {
        let rs = emit_expr(r);
        match method {
            "nil?" => return format!("({rs} == nil)"),
            "to_s" => return format!("\"\\({rs})\""),
            "to_i" => return format!("Int(\"\\({rs})\")!"),
            "to_f" => return format!("Double(\"\\({rs})\")!"),
            "empty?" => return format!("{rs}.isEmpty"),
            "any?" => return format!("!{rs}.isEmpty"),
            "length" | "size" => return format!("{rs}.count"),
            "upcase" => return format!("{rs}.uppercased()"),
            "downcase" => return format!("{rs}.lowercased()"),
            "strip" => {
                return format!("{rs}.trimmingCharacters(in: .whitespacesAndNewlines)")
            }
            // Identity no-ops on Swift value types; `to_h` only on an
            // actual Hash (elsewhere it's a real method).
            "to_a" | "dup" | "freeze" => return rs,
            "join" => return format!("{rs}.joined(separator: \"\")"),
            "keys" if recv_is_hash(r) => return format!("Array({rs}.keys)"),
            "values" if recv_is_hash(r) => return format!("Array({rs}.values)"),
            "to_h"
                if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
                    || matches!(
                        r.ty.as_ref(),
                        Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Hash"
                    ) =>
            {
                return rs;
            }
            _ => {}
        }
        // Self-receiver: a zero-arg send is a CALL by default (the
        // Kotlin keystone fix) — property read ONLY when the name is a
        // known property of the class being emitted or an ancestor.
        if matches!(&*r.node, ExprNode::SelfRef) {
            let name = camel(method);
            let is_prop = INSTANCE_PROP_TYPES.with(|m| m.borrow().contains_key(&name))
                || CURRENT_CLASS.with(|c| ancestor_has_prop(&c.borrow(), &name));
            if is_prop {
                if NONNULL_PROPS.with(|s| s.borrow().contains(&name)) {
                    return format!("self.{name}!");
                }
                return format!("self.{name}");
            }
            return format!("self.{name}()");
        }
        // A `Const` receiver (a class / namespace like `Db`) means a
        // 0-arg *method* call — unless it's a registered object-level
        // accessor (`ActiveRecord.adapter`), which reads as a property.
        // A receiver whose class type registers this name as a real
        // instance method keeps its parens too.
        if matches!(&*r.node, ExprNode::Const { .. }) {
            let name = camel(method);
            if OBJECT_PROPS.with(|m| m.borrow().contains(&format!("{rs}.{name}"))) {
                return format!("{rs}.{name}");
            }
            let try_kw = if throws_lookup(&rs, &name) { "try " } else { "" };
            return format!("{try_kw}{rs}.{name}()");
        }
        if is_known_instance_method(r, method) {
            return format!("{rs}.{}()", camel(method));
        }
        // A typed receiver reading a property the class (or an ancestor)
        // declares — including collapsed predicate readers
        // (`article.persisted?` → Base's `persisted` var).
        if let Some(crate::ty::Ty::Class { id, .. }) = r.ty.as_ref() {
            let cls = super::naming::type_name(id.0.as_str());
            let name = camel(method);
            let has_prop = CLASS_PROPS
                .with(|m| m.borrow().get(&cls).map_or(false, |s| s.contains(&name)))
                || ancestor_has_prop(&cls, &name);
            if has_prop {
                return format!("{rs}.{name}");
            }
        }
        if !forces_parens(method) && !method.ends_with('?') && !method.ends_with('!') {
            // Attribute read on an instance.
            return format!("{rs}.{}", camel(method));
        }
    }

    // Block → Swift trailing closure (`.each` → `.forEach`).
    if let Some(b) = block {
        let sw_method = if method == "each" { "forEach".to_string() } else { camel(method) };
        let lam = emit_expr(b);
        let base = match recv {
            Some(r) => format!("{}.{sw_method}", emit_expr(r)),
            None => sw_method,
        };
        if args_s.is_empty() {
            return format!("{base} {lam}");
        }
        return format!("{base}({}) {lam}", args_s.join(", "));
    }

    // Stdlib-bridging Const receivers (the Kotlin special cases).
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            let joined = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::");
            if joined == "Base64" && method == "strict_encode64" && args.len() == 1 {
                return format!("Data(({}).utf8).base64EncodedString()", args_s[0]);
            }
            if joined == "JSON" && method == "generate" && args.len() == 1 {
                return format!("JsonBuilder.encodeValue({})", args_s[0]);
            }
        }
    }

    // A bare zero-arg send naming a parameter of the current method is
    // that parameter (the view lowerer's partial-local Send shape).
    if recv.is_none() && args.is_empty() && block.is_none() && is_param(method) {
        return camel(method);
    }

    // General call — with `try` when the callee is registered throwing
    // (Const-receiver statics resolve through the ancestor walk;
    // typed-Var receivers through their class), and the kwargs-splat
    // decision on the rendered arguments.
    let name = camel(method);
    match recv {
        Some(r) => {
            let rs = emit_expr(r);
            let recv_type = match &*r.node {
                ExprNode::Const { .. } => Some(rs.clone()),
                _ => match r.ty.as_ref() {
                    Some(crate::ty::Ty::Class { id, .. }) => {
                        Some(super::naming::type_name(id.0.as_str()))
                    }
                    _ => None,
                },
            };
            let try_kw = recv_type
                .map_or(false, |t| throws_lookup(&t, &name))
                .then_some("try ")
                .unwrap_or("");
            format!("{try_kw}{rs}.{name}({})", emit_call_args(recv, method, args))
        }
        None => format!("{name}({})", emit_call_args(recv, method, args)),
    }
}
