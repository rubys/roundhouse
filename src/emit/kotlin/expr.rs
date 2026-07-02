//! `Expr` → Kotlin source.
//!
//! Phase 2 coverage: the node kinds the lowered model bodies exercise.
//! Modeled on `src/emit/crystal/expr.rs` but rendered Kotlin-idiomatic —
//! camelCase identifiers (`super::naming::camel`), `?:` for nil-coalescing
//! `||`, `when` for `case`, trailing lambdas for blocks, and `var`/`val`
//! inference for local assignments.
//!
//! Untyped/edge nodes that don't map cleanly emit a `/* TODO kind */`
//! marker rather than panicking, so a full model still renders and the
//! gaps are visible in the output.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::expr::{
    Arm, BoolOpKind, Expr, ExprNode, InterpPart, IrHint, LValue, Literal, OpAssignOp, Pattern,
};

use super::naming::{camel, type_name};
use super::ty::kotlin_ty;

thread_local! {
    /// Local names already declared in the current method body (so the
    /// first `Assign` emits `val`/`var` and later ones emit bare `=`).
    static DECLARED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Local names assigned more than once → declared `var` (else `val`).
    static REASSIGNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// For locals first assigned `nil`, the nullable Kotlin type taken
    /// from a later non-nil assignment — so `var x = null` (which Kotlin
    /// infers as `Nothing?`) becomes `var x: T? = null`.
    static NIL_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// For locals first assigned an empty `{}`/`[]`, the element type
    /// inferred from later `map[k]=v` / `list << x` — so the empty literal
    /// gets a precise declared type instead of `<Any?>`.
    static CONTAINER_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// When set, `return` emits `return@<label>` — used for `initialize`
    /// bodies wrapped in `run { }` (Kotlin `init` blocks can't `return`).
    static RETURN_LABEL: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    /// Whether the method currently being emitted returns `Unit`. A guard
    /// `return nil` in a void method must emit a bare `return` — Kotlin's
    /// `Unit` can't carry a `null` value (`return null` is a type error).
    static RETURNS_UNIT: RefCell<bool> = const { RefCell::new(false) };
    /// camelCased names of the current class's accessor-backed properties
    /// (`attr_*` + body ivars). A zero-arg `self`-receiver send resolves to
    /// a Kotlin property read only when its name is in here; everything else
    /// is a method call needing `()`. Empty for `object`s (modules), whose
    /// self-sends are always method calls.
    static INSTANCE_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// camelCased parameter names of the method currently being emitted. A
    /// zero-arg, no-receiver `Send` whose name is in here is a reference to
    /// the parameter, not a self-method call — emit the bare identifier
    /// without `()`. (The view lowerer represents a partial local like
    /// `article` as a bare implicit-self `Send` in argument position but as
    /// a `Var` in receiver position; this reconciles the two.)
    static PARAM_NAMES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Instance property name → declared `Ty`, so a `self.col = <Any?>`
    /// write (the `assign_from_row`/`initialize`/`update` column shape,
    /// where the RHS is an untyped `row[k]`/`attrs[k]` lookup) can coerce
    /// the value to the column's scalar type. Kotlin won't assign `Any?`
    /// to a `Long`/`String` slot. Set per class beside `INSTANCE_PROPS`.
    static INSTANCE_PROP_TYPES: RefCell<HashMap<String, crate::ty::Ty>> =
        RefCell::new(HashMap::new());
    /// `"Object.prop"` keys for module/object-level accessor properties
    /// (`class << self; attr_accessor :adapter` → `ActiveRecord.adapter`).
    /// A `Const`-receiver zero-arg send keyed here reads as a property
    /// (`ActiveRecord.adapter`) instead of a call. Populated by a pre-scan
    /// of all runtime classes before rendering (see
    /// `library::register_object_accessors`).
    static OBJECT_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Name of the class currently being emitted, so an implicit-self
    /// `new(attrs)` (a companion factory like `Base.create`) resolves to
    /// the Kotlin constructor `Base(attrs)`. Empty for object/module emit.
    static CURRENT_CLASS: RefCell<String> = const { RefCell::new(String::new()) };
    /// Class hierarchy: simple class name → (parent simple name, instance
    /// member names). Populated by a pre-scan of every runtime + model class
    /// before any model renders, so a subclass can mark `override` on the
    /// members it inherits (Kotlin requires explicit `override`, unlike
    /// TS/Crystal). See `library::register_class_hierarchy`.
    static CLASS_HIERARCHY: RefCell<HashMap<String, (Option<String>, HashSet<String>)>> =
        RefCell::new(HashMap::new());
    /// Class simple name → camelCased names of its *zero-arg instance
    /// methods* (excludes `attr_*` / body-ivar properties). A zero-arg send
    /// to a typed-`Class` receiver whose member is in this set (walking
    /// ancestors) is a Kotlin method call and keeps its `()`; members not
    /// listed default to property-read form. Lets `article.comments`
    /// (has-many loader method) emit `article.comments()` while
    /// `article.title` (column property) stays `article.title`.
    static CLASS_INSTANCE_METHODS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// `"Receiver.method"` → the callee's camelCased parameter names. Used
    /// to decide whether a call-site `kwargs:true` hash splats into Kotlin
    /// named arguments (`truncate(body, length = 100)`, when the keys are a
    /// subset of these params) or stays a map literal (`Broadcasts.append`,
    /// whose lone param is a `Map`). An unregistered receiver (e.g. the
    /// hand-written `Broadcasts` primitive) falls back to the map literal —
    /// the safe default.
    static METHOD_PARAMS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// Full `var <name>: <T> = <default>` declarations for locals whose
    /// first assignment is inside a nested scope (an `if`/loop/block) yet are
    /// also assigned at an outer level — Kotlin would scope the in-branch
    /// `var` to that branch, leaving the outer assignment unresolved. These
    /// hoist to the method top (emitted by `library::emit_method`); the
    /// names are pre-seeded into `DECLARED` so every assignment is a bare `=`.
    static HOISTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// camelCased instance-property names proven non-null by an enclosing
    /// `if (!prop.nil?)` guard — read with a `!!` so Kotlin accepts them in a
    /// non-null position (it won't smart-cast a mutable property). Scoped to
    /// the guarded branch by `emit_if`.
    static NONNULL_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// camelCased `@ivar` names of the current `object`/module that hold
    /// mutable singleton state (e.g. ViewHelpers' `@slots` content_for store).
    /// In a concurrent server (Javalin dispatches each request on its own
    /// thread) such process-global state would bleed across requests, so it's
    /// emitted as a `ThreadLocal`: reads become `name.get()`, whole-reassigns
    /// `name.set(…)`. Empty for instance classes (their ivars are per-object).
    static OBJECT_TL_FIELDS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Install the current object's thread-local field names (see
/// `OBJECT_TL_FIELDS`); empty for instance-class emit.
pub(super) fn set_object_tl_fields(names: HashSet<String>) {
    OBJECT_TL_FIELDS.with(|f| *f.borrow_mut() = names);
}

fn is_object_tl_field(name: &str) -> bool {
    OBJECT_TL_FIELDS.with(|f| f.borrow().contains(name))
}

/// Append `!!` to a property read proven non-null by an enclosing guard.
fn nonnull_read(name: String) -> String {
    if NONNULL_PROPS.with(|p| p.borrow().contains(&name)) {
        format!("{name}!!")
    } else {
        name
    }
}

/// camelCased name of a property/local read expression (`@x` or `x`), when
/// it names a known instance property — else `None`.
fn read_prop_name(e: &Expr) -> Option<String> {
    let name = match &*e.node {
        ExprNode::Ivar { name } => camel(name.as_str()),
        ExprNode::Var { name, .. } => camel(name.as_str()),
        // An accessor read lowers to a zero-arg (implicit-)self send.
        ExprNode::Send { recv, method, args, .. }
            if args.is_empty()
                && matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef)) =>
        {
            camel(method.as_str())
        }
        _ => return None,
    };
    is_instance_prop(&name).then_some(name)
}

/// Properties a `<prop>.nil?` test names, when `prop` is a known instance
/// property. A bare nil-test proves the prop non-null in the *else* branch;
/// negated (`!prop.nil?`) proves it in the *then* branch.
fn nil_test_prop(e: &Expr) -> Option<String> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if method.as_str() == "nil?" && args.is_empty() {
            return read_prop_name(r);
        }
    }
    None
}

/// Collect the props an `if` condition proves non-null in each branch:
/// `then_nn` (the cond holds) and `else_nn` (it doesn't). Handles
/// `!prop.nil?` (→ then), `prop.nil?` (→ else), and `&&` of such.
fn guarded_nonnull(cond: &Expr, then_nn: &mut Vec<String>, else_nn: &mut Vec<String>) {
    // Logical-not has two IR shapes: `Send{recv:None, "!", [x]}` and the
    // postfix `Send{recv:Some(x), "!", []}`. Extract the negated operand.
    let negated = match &*cond.node {
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "!" && args.len() == 1 =>
        {
            Some(&args[0])
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            Some(r)
        }
        _ => None,
    };
    if let Some(inner) = negated {
        if let Some(n) = nil_test_prop(inner) {
            then_nn.push(n);
        }
        return;
    }
    match &*cond.node {
        ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } => {
            guarded_nonnull(left, then_nn, else_nn);
            guarded_nonnull(right, then_nn, else_nn);
        }
        _ => {
            if let Some(n) = nil_test_prop(cond) {
                else_nn.push(n);
            }
        }
    }
}

/// The hoisted-var declaration lines computed for the current method (see
/// `HOISTED`), to be emitted at the top of the body.
pub(super) fn hoisted_decls() -> Vec<String> {
    HOISTED.with(|h| h.borrow().clone())
}

/// Clear the method-param registry (start of an `emit` run).
pub(super) fn reset_method_params() {
    METHOD_PARAMS.with(|m| m.borrow_mut().clear());
}

/// Register `Receiver.method` → its camelCased parameter names.
pub(super) fn register_method_params(receiver: &str, method: &str, params: HashSet<String>) {
    METHOD_PARAMS.with(|m| {
        m.borrow_mut().insert(format!("{receiver}.{}", camel(method)), params);
    });
}

fn method_params_lookup(receiver: &str, method: &str) -> Option<HashSet<String>> {
    METHOD_PARAMS.with(|m| m.borrow().get(&format!("{receiver}.{}", camel(method))).cloned())
}

/// Resolve the callee's parameter names for a `recv.method` (or self-send)
/// call: a `Const` receiver keys directly; a `self`/implicit receiver walks
/// the current class up its ancestor chain (so a controller's `this.render`
/// finds `ActionControllerBase.render`). `None` if unregistered.
fn method_params_for(recv: Option<&Expr>, method: &str) -> Option<HashSet<String>> {
    match recv.map(|r| &*r.node) {
        Some(ExprNode::Const { path }) => {
            let name = type_name(&path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"));
            method_params_lookup(&name, method)
        }
        None | Some(ExprNode::SelfRef) => {
            let mut cur = Some(CURRENT_CLASS.with(|c| c.borrow().clone()));
            let mut guard = 0;
            while let Some(c) = cur {
                guard += 1;
                if c.is_empty() || guard > 32 {
                    break;
                }
                if let Some(p) = method_params_lookup(&c, method) {
                    return Some(p);
                }
                cur = CLASS_HIERARCHY
                    .with(|h| h.borrow().get(&c).and_then(|(parent, _)| parent.clone()));
            }
            None
        }
        _ => None,
    }
}

/// True when the resolved callee has every `key` (camelCased) among its
/// parameters — i.e. a kwargs hash can splat to named args.
fn kwargs_match_params(recv: Option<&Expr>, method: &str, keys: &[String]) -> bool {
    method_params_for(recv, method).map(|p| keys.iter().all(|k| p.contains(k))).unwrap_or(false)
}

/// Register a class's zero-arg instance-method names (camelCased) for the
/// typed-receiver call-vs-property decision. See `CLASS_INSTANCE_METHODS`.
pub(super) fn register_instance_methods(name: &str, methods: HashSet<String>) {
    CLASS_INSTANCE_METHODS.with(|m| m.borrow_mut().insert(name.to_string(), methods));
}

/// True when `method` (camelCased) is a known zero-arg instance method of
/// `class_name` or any ancestor.
/// The declared `Ty` of an instance property by camelCased name (incl.
/// body-ivar types plumbed in by `emit_library_class`); `None` if unknown.
fn instance_prop_ty(name: &str) -> Option<crate::ty::Ty> {
    INSTANCE_PROP_TYPES.with(|t| t.borrow().get(&camel(name)).cloned())
}

/// A receiver's effective element-container kind: its own `Ty` (through a
/// nullable `Union`), or — for a bare `@ivar`/local read — the declared type
/// of the same-named instance property. Lets `data.keys`/`data.length`
/// dispatch as a Map when `data` is a `Hash`-typed body ivar whose read node
/// carries no precise type.
fn recv_is_hash(r: &Expr) -> bool {
    if ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Hash { .. })) {
        return true;
    }
    matches!(&*r.node,
        ExprNode::Ivar { name } | ExprNode::Var { name, .. }
            if matches!(instance_prop_ty(name.as_str()), Some(crate::ty::Ty::Hash { .. })))
}

fn recv_is_array(r: &Expr) -> bool {
    if ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Array { .. })) {
        return true;
    }
    matches!(&*r.node,
        ExprNode::Ivar { name } | ExprNode::Var { name, .. }
            if matches!(instance_prop_ty(name.as_str()), Some(crate::ty::Ty::Array { .. })))
}

fn is_instance_method_of(class_name: &str, method: &str) -> bool {
    let cm = camel(method);
    let mut cur = Some(class_name.to_string());
    let mut guard = 0;
    while let Some(name) = cur {
        guard += 1;
        if guard > 32 {
            break;
        }
        let found = CLASS_INSTANCE_METHODS.with(|m| {
            m.borrow().get(&name).map(|set| set.contains(&cm)).unwrap_or(false)
        });
        if found {
            return true;
        }
        cur = CLASS_HIERARCHY
            .with(|h| h.borrow().get(&name).and_then(|(parent, _)| parent.clone()));
    }
    false
}

/// The simple class name of a receiver expression's type, when it's a
/// `Ty::Class` (its last `::` segment). Used to consult the instance-method
/// registry for the call-vs-property decision.
fn receiver_class_name(r: &Expr) -> Option<String> {
    match r.ty.as_ref()? {
        crate::ty::Ty::Class { id, .. } => Some(type_name(id.0.as_str())),
        _ => None,
    }
}

/// Clear the class-hierarchy registry (start of an `emit` run).
pub(super) fn reset_class_hierarchy() {
    CLASS_HIERARCHY.with(|h| h.borrow_mut().clear());
}

/// Register a class's parent + instance-member-name set for override
/// resolution. `members` are the camelCased instance member names
/// (`[]`→`get`, `[]=`→`set`).
pub(super) fn register_class_hierarchy(name: &str, parent: Option<&str>, members: HashSet<String>) {
    CLASS_HIERARCHY
        .with(|h| h.borrow_mut().insert(name.to_string(), (parent.map(str::to_string), members)));
}

/// The instance member names visible from `class_name` upward — its own
/// members unioned with all ancestors'. Call with a class's *parent* name
/// to get the set a member must be in to need an `override` modifier.
/// Unknown classes (e.g. `RuntimeException`) contribute nothing.
pub(super) fn ancestor_members(class_name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = Some(class_name.to_string());
    let mut guard = 0;
    while let Some(name) = cur {
        guard += 1;
        if guard > 32 {
            break; // cycle guard
        }
        let next = CLASS_HIERARCHY.with(|h| {
            h.borrow().get(&name).map(|(parent, members)| {
                out.extend(members.iter().cloned());
                parent.clone()
            })
        });
        match next {
            Some(parent) => cur = parent,
            None => break,
        }
    }
    out
}

/// The instance *property* names visible from `class_name` upward — each
/// ancestor's members minus its methods. A controller's `self.request_format`
/// (a property defined on `ActionController::Base`) must read as a property,
/// not a `()` call, even though it isn't in the subclass's own prop set.
pub(super) fn ancestor_props(class_name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = Some(class_name.to_string());
    let mut guard = 0;
    while let Some(name) = cur {
        guard += 1;
        if guard > 32 {
            break;
        }
        let (members, parent) = CLASS_HIERARCHY.with(|h| {
            h.borrow()
                .get(&name)
                .map(|(p, m)| (m.clone(), p.clone()))
                .unwrap_or_default()
        });
        let methods = CLASS_INSTANCE_METHODS.with(|m| m.borrow().get(&name).cloned().unwrap_or_default());
        out.extend(members.into_iter().filter(|m| !methods.contains(m)));
        cur = parent;
    }
    out
}

/// Set the class name used to resolve implicit-self `new` (see
/// `CURRENT_CLASS`); `""` disables the rewrite.
pub(super) fn set_current_class(name: &str) {
    CURRENT_CLASS.with(|c| *c.borrow_mut() = name.to_string());
}

/// Clear the object-accessor registry (start of an `emit` run).
pub(super) fn reset_object_accessors() {
    OBJECT_PROPS.with(|p| p.borrow_mut().clear());
}

/// Register `Object.prop` as a module/object-level property read.
pub(super) fn register_object_accessor(object: &str, prop: &str) {
    OBJECT_PROPS.with(|p| p.borrow_mut().insert(format!("{object}.{}", camel(prop))));
}

fn is_object_prop(object: &str, method: &str) -> bool {
    OBJECT_PROPS.with(|p| p.borrow().contains(&format!("{object}.{}", camel(method))))
}

/// Install the current class's property-name set (see `INSTANCE_PROPS`).
/// Called by `library::emit_library_class` before emitting method bodies;
/// reset to empty for `object`/module emission.
pub(super) fn set_instance_props(props: HashSet<String>) {
    INSTANCE_PROPS.with(|p| *p.borrow_mut() = props);
}

/// Install the current class's property name → `Ty` map (see
/// `INSTANCE_PROP_TYPES`); empty for object/module emission.
pub(super) fn set_instance_prop_types(types: HashMap<String, crate::ty::Ty>) {
    INSTANCE_PROP_TYPES.with(|t| *t.borrow_mut() = types);
}

/// The coercion target for a `self.<prop> = …` write: the prop's declared
/// `Ty` when it's a scalar column type (`Long`/`String`/…) the emitter can
/// convert an `Any?` value into. Returns `None` for `Any?`/object props (no
/// coercion) and unknown props.
fn instance_prop_scalar_ty(method: &str) -> Option<crate::ty::Ty> {
    use crate::ty::Ty;
    INSTANCE_PROP_TYPES.with(|t| {
        t.borrow().get(&camel(method)).and_then(|ty| match ty {
            Ty::Int | Ty::Float | Ty::Str | Ty::Sym | Ty::Bool => Some(ty.clone()),
            _ => None,
        })
    })
}

fn is_instance_prop(method: &str) -> bool {
    INSTANCE_PROPS.with(|p| p.borrow().contains(&camel(method)))
}

/// Install the current method's parameter-name set (see `PARAM_NAMES`).
/// Called by `library::emit_method` (and the `init`-block path) before the
/// body renders.
pub(super) fn set_param_names(names: HashSet<String>) {
    PARAM_NAMES.with(|p| *p.borrow_mut() = names);
}

fn is_param(method: &str) -> bool {
    PARAM_NAMES.with(|p| p.borrow().contains(&camel(method)))
}

/// Set the active labeled-return target (`None` = plain `return`).
pub(super) fn set_return_label(label: Option<&'static str>) {
    RETURN_LABEL.with(|r| *r.borrow_mut() = label);
}

/// Record whether the method being emitted returns `Unit` (see
/// `RETURNS_UNIT`).
pub(super) fn set_returns_unit(b: bool) {
    RETURNS_UNIT.with(|r| *r.borrow_mut() = b);
}

/// Reset per-method local-decl tracking and pre-scan the body for
/// reassignment counts. Called by `library::emit_method` before the body
/// is rendered.
pub(super) fn begin_method(body: &Expr) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut nil_types: HashMap<String, String> = HashMap::new();
    count_assigns(body, &mut counts, &mut nil_types);
    DECLARED.with(|d| d.borrow_mut().clear());
    REASSIGNED.with(|r| {
        let mut set = r.borrow_mut();
        set.clear();
        for (name, n) in counts {
            if n > 1 {
                set.insert(name);
            }
        }
    });
    NIL_TYPES.with(|t| *t.borrow_mut() = nil_types);
    set_return_label(None);

    let mut container_types: HashMap<String, String> = HashMap::new();
    scan_container_types(body, &mut container_types);
    CONTAINER_TYPES.with(|t| *t.borrow_mut() = container_types);

    // Locals first assigned in a nested scope but also assigned higher up
    // must hoist to the method top (else the in-branch `var` is out of scope
    // for the outer assignment). Pre-seed `DECLARED` so all their writes are
    // bare, and stash the declaration lines for `emit_method` to prepend.
    let hoist = scan_hoist(body);
    DECLARED.with(|d| {
        let mut set = d.borrow_mut();
        for (n, _) in &hoist {
            set.insert(n.clone());
        }
    });
    HOISTED.with(|h| {
        *h.borrow_mut() =
            hoist.iter().map(|(n, ty)| format!("var {n}: {ty} = {}", kt_default(ty))).collect();
    });
}

/// Default initializer for a Kotlin type *name* (string form), for hoisted
/// `var` declarations. Nullable types and unknowns default to `null`.
fn kt_default(ty: &str) -> &'static str {
    match ty {
        "Long" | "Int" => "0L",
        "Double" => "0.0",
        "Boolean" => "false",
        "String" => "\"\"",
        _ if ty.ends_with('?') => "null",
        _ if ty.starts_with("MutableList") => "mutableListOf()",
        _ if ty.starts_with("MutableMap") => "mutableMapOf()",
        _ => "null",
    }
}

/// Find locals needing top-of-method hoisting: first assigned at a nested
/// scope depth (>0) yet assigned more than once. Returns `(name, kotlin_ty)`
/// pairs, the type taken from the first assignment's value.
fn scan_hoist(body: &Expr) -> Vec<(String, String)> {
    // name → (first-assignment depth, total assignment count, type)
    let mut info: std::collections::BTreeMap<String, (usize, usize, String)> =
        std::collections::BTreeMap::new();
    walk_hoist(body, 0, &mut info);
    info.into_iter()
        .filter(|(_, (depth, count, _))| *depth > 0 && *count > 1)
        .map(|(n, (_, _, ty))| (n, ty))
        .collect()
}

fn walk_hoist(
    e: &Expr,
    depth: usize,
    info: &mut std::collections::BTreeMap<String, (usize, usize, String)>,
) {
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
        let n = camel(name.as_str());
        let ty = match value.ty.as_ref() {
            Some(t) if !matches!(t, crate::ty::Ty::Nil) => kotlin_ty(t),
            _ => "Any?".to_string(),
        };
        let entry = info.entry(n).or_insert((depth, 0, ty));
        entry.1 += 1;
    }
    // Recurse, deepening only into genuinely nested scopes (branch/loop/block
    // bodies); a `Seq` and an `if` condition stay at the current depth.
    match &*e.node {
        ExprNode::Seq { exprs } => {
            for c in exprs {
                walk_hoist(c, depth, info);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_hoist(cond, depth, info);
            walk_hoist(then_branch, depth + 1, info);
            walk_hoist(else_branch, depth + 1, info);
        }
        ExprNode::While { cond, body, .. } => {
            walk_hoist(cond, depth, info);
            walk_hoist(body, depth + 1, info);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_hoist(scrutinee, depth, info);
            for a in arms {
                walk_hoist(&a.body, depth + 1, info);
            }
        }
        ExprNode::Lambda { body, .. } => walk_hoist(body, depth + 1, info),
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            walk_hoist(value, depth, info)
        }
        _ => {
            for c in children(e) {
                walk_hoist(c, depth, info);
            }
        }
    }
}

/// Infer element types for empty-container locals from how they're later
/// populated: `map[k] = v` → `MutableMap<K, V>`; `list << x` → `MutableList<E>`.
fn scan_container_types(e: &Expr, out: &mut HashMap<String, String>) {
    // Element/key/value types from writes. The IR types array-index reads
    // conservatively as nilable (Ruby OOB → nil), but Kotlin's list/map
    // operators return non-null, so strip the top-level nullability.
    let nn = |ty: Option<&crate::ty::Ty>| -> String {
        match ty {
            Some(crate::ty::Ty::Union { variants }) => {
                let nn: Vec<&crate::ty::Ty> =
                    variants.iter().filter(|t| !matches!(t, crate::ty::Ty::Nil)).collect();
                if nn.len() == 1 {
                    kotlin_ty(nn[0])
                } else {
                    "Any?".to_string()
                }
            }
            Some(t) => kotlin_ty(t),
            None => "Any?".to_string(),
        }
    };
    match &*e.node {
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => {
            if let ExprNode::Var { name, .. } = &*recv.node {
                out.entry(camel(name.as_str()))
                    .or_insert(format!("MutableMap<{}, {}>", nn(index.ty.as_ref()), nn(value.ty.as_ref())));
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if matches!(method.as_str(), "<<" | "add" | "push") && args.len() == 1 =>
        {
            if let ExprNode::Var { name, .. } = &*r.node {
                out.entry(camel(name.as_str()))
                    .or_insert(format!("MutableList<{}>", nn(args[0].ty.as_ref())));
            }
        }
        // `map[k] = v` lowered as a Send `[]=`.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Var { name, .. } = &*r.node {
                out.entry(camel(name.as_str()))
                    .or_insert(format!("MutableMap<{}, {}>", nn(args[0].ty.as_ref()), nn(args[1].ty.as_ref())));
            }
        }
        _ => {}
    }
    for child in children(e) {
        scan_container_types(child, out);
    }
}

fn count_assigns(
    e: &Expr,
    counts: &mut HashMap<String, usize>,
    nil_types: &mut HashMap<String, String>,
) {
    // A compound assignment always mutates → force `var`.
    if let ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } = &*e.node {
        *counts.entry(camel(name.as_str())).or_insert(0) += 2;
    }
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
        let cn = camel(name.as_str());
        *counts.entry(cn.clone()).or_insert(0) += 1;
        // Record the first non-nil assigned type so a `nil`-first local
        // gets a real nullable declaration type.
        if !nil_types.contains_key(&cn) {
            if let Some(ty) = value.ty.as_ref() {
                if !matches!(ty, crate::ty::Ty::Nil) {
                    let mut kt = kotlin_ty(ty);
                    if !kt.ends_with('?') {
                        kt.push('?');
                    }
                    nil_types.insert(cn, kt);
                }
            }
        }
    }
    for child in children(e) {
        count_assigns(child, counts, nil_types);
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
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => v.push(value),
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

/// The view lowerer builds HTML by accumulating into a string buffer
/// (`io = String.new; io << chunk; …; io`), tagging the three sites with
/// `IrHint`s. Kotlin uses a `StringBuilder`:
///   - `Init`   `io = String.new`  → `val io = StringBuilder()`
///   - `Append` `io << chunk`      → `io.append(chunk)`
///   - `Result` terminal `io`      → `io.toString()`
/// Mirrors `crystal::expr::try_string_builder`. Non-hinted sites fall
/// through to the normal node emit.
fn try_string_builder(e: &Expr) -> Option<String> {
    match e.hint? {
        IrHint::StringBuilderInit => {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                return Some(format!("val {} = StringBuilder()", camel(name.as_str())));
            }
            None
        }
        IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        return Some(format!(
                            "{}.append({})",
                            camel(name.as_str()),
                            emit_expr(&args[0])
                        ));
                    }
                }
            }
            None
        }
        IrHint::StringBuilderResult => {
            if let ExprNode::Var { name, .. } = &*e.node {
                return Some(format!("{}.toString()", camel(name.as_str())));
            }
            None
        }
    }
}

pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

/// Emit a top-level constant's value. A non-empty hash/array literal drops
/// its explicit type args so Kotlin infers the precise element type (e.g.
/// `STATUS_CODES` is `MutableMap<String, Long>`). Safe for a constant — it's
/// never passed where map/list invariance against `Any?` would bite.
pub fn emit_constant_for_runtime(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Hash { entries, .. } if !entries.is_empty() => emit_hash_inferred(entries),
        ExprNode::Array { elements, .. } if !elements.is_empty() => {
            let els: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("mutableListOf({})", els.join(", "))
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

fn emit_node(n: &ExprNode, e: &Expr) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => nonnull_read(camel(name.as_str())),
        // Instance variable → property reference. A thread-local singleton
        // field reads through `.get()`.
        ExprNode::Ivar { name } => {
            let n = camel(name.as_str());
            if is_object_tl_field(&n) {
                format!("{n}.get()")
            } else {
                nonnull_read(n)
            }
        }
        ExprNode::SelfRef => "this".to_string(),
        // Classes/modules are emitted flat in `package roundhouse`, so a
        // qualified ref (`ActionDispatch::Router::MatchResult`) resolves
        // by its last segment — except colliding framework bases, which
        // `type_name` disambiguates (`ActiveRecord::Base` → `ActiveRecordBase`).
        ExprNode::Const { path } => {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            type_name(&joined)
        }
        ExprNode::Hash { entries, .. } => emit_hash(entries, e),
        ExprNode::Array { elements, .. } => emit_array(elements, e),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, left, right, .. } => emit_bool_op(*op, left, right, e),
        ExprNode::Send { recv, method, args, block, .. } => {
            let rendered = emit_send(recv.as_ref(), method.as_str(), args, block.as_ref());
            coerce_nullable_finder(rendered, recv.as_ref(), method.as_str(), e.ty.as_ref())
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            emit_if(cond, then_branch, else_branch)
        }
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms),
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(emit_expr)
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::Assign { target, value } => emit_assign(target, value),
        ExprNode::OpAssign { target, op, value } => emit_op_assign(target, *op, value),
        ExprNode::Return { value } => {
            // `return nil` → `return null`, except in a `Unit` method where
            // it's a bare `return` (Kotlin `Unit` can't carry `null`).
            // Inside an `init`-block `run {}` wrapper the return is labeled
            // (`return@run`).
            let label = RETURN_LABEL.with(|r| *r.borrow());
            let nil_in_unit = RETURNS_UNIT.with(|r| *r.borrow())
                && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
            if nil_in_unit {
                return match label {
                    Some(label) => format!("return@{label}"),
                    None => "return".to_string(),
                };
            }
            let v = emit_expr(value);
            match label {
                Some(label) => format!("return@{label} {v}"),
                None => format!("return {v}"),
            }
        }
        ExprNode::While { cond, body, until_form } => {
            let c = emit_expr(cond);
            let c = if *until_form { format!("!({c})") } else { c };
            format!("while ({c}) {{\n{}\n}}", indent(&emit_expr(body)))
        }
        ExprNode::Raise { value } => emit_raise(value),
        // `super()` in `initialize` has no Kotlin method-body analog
        // (super-constructor calls live in the class header). Phase 2
        // emits a placeholder; Phase 3 wires the base properly.
        ExprNode::Super { .. } => "/* super() */".to_string(),
        ExprNode::Cast { value, target_ty } => emit_cast(value, target_ty),
        ExprNode::Lambda { params, body, .. } => emit_lambda(params, body, false),
        // `yield a, b` → invoke the synthesized `block` parameter (see
        // `library::emit_method`, which adds it to yielding methods).
        ExprNode::Yield { args } => {
            format!("block({})", args.iter().map(emit_expr).collect::<Vec<_>>().join(", "))
        }
        ExprNode::RescueModifier { expr, fallback } => format!(
            "try {{ {} }} catch (e: Exception) {{ {} }}",
            emit_expr(expr),
            emit_expr(fallback)
        ),
        other => format!("/* TODO {} */", other.kind_str()),
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        // `Ty::Int → Long`, and Kotlin won't compare/assign across
        // numeric types, so integer literals carry the `L` suffix. (The
        // hand-written `Db` primitive correspondingly takes `Long`
        // indices.)
        Literal::Int { value } => format!("{value}L"),
        Literal::Float { value } => {
            if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                format!("{value}")
            }
        }
        Literal::Str { value } => format!("\"{}\"", escape_str(value)),
        // No symbol type in Kotlin → string.
        Literal::Sym { value } => format!("\"{}\"", escape_str(value.as_str())),
        Literal::Regex { pattern, .. } => format!("Regex(\"{}\")", escape_str(pattern)),
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            // Kotlin has no `\f` escape; use the unicode form.
            '\u{0C}' => out.push_str("\\u000C"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

fn emit_hash(entries: &[(Expr, Expr)], e: &Expr) -> String {
    if entries.is_empty() {
        if let Some(crate::ty::Ty::Hash { key, value }) = e.ty.as_ref() {
            return format!("mutableMapOf<{}, {}>()", kotlin_ty(key), kotlin_ty(value));
        }
        return "mutableMapOf<String, Any?>()".to_string();
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("{} to {}", emit_expr(k), emit_expr(v)))
        .collect();
    // Heterogeneous `<String, Any?>` so a mixed map (Broadcasts payloads,
    // render options) type-checks against `Any?` params. A homogeneous map in
    // return position is re-typed by `wrap_return` (where map invariance is a
    // non-issue) to match its declared return type.
    format!("mutableMapOf<String, Any?>({})", pairs.join(", "))
}

/// Emit a non-empty hash literal with no explicit type arguments, so the
/// surrounding context (a method's declared `MutableMap<K, V>` return type)
/// drives inference. Safe only where invariance can't bite — i.e. `return`
/// position, not an argument.
fn emit_hash_inferred(entries: &[(Expr, Expr)]) -> String {
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("{} to {}", emit_expr(k), emit_expr(v)))
        .collect();
    format!("mutableMapOf({})", pairs.join(", "))
}

/// Public entry for `wrap_return` (library.rs): render an expression for a
/// `return`, re-typing a non-empty hash literal so the declared return type
/// is inferred rather than the literal's heterogeneous `<String, Any?>`.
pub(super) fn emit_return_value(e: &Expr) -> String {
    if let ExprNode::Hash { entries, .. } = &*e.node {
        if !entries.is_empty() {
            return emit_hash_inferred(entries);
        }
    }
    emit_expr(e)
}

fn emit_array(elements: &[Expr], e: &Expr) -> String {
    if elements.is_empty() {
        // Use the annotated element type when present, else Any?.
        if let Some(crate::ty::Ty::Array { elem }) = e.ty.as_ref() {
            return format!("mutableListOf<{}>()", kotlin_ty(elem));
        }
        return "mutableListOf<Any?>()".to_string();
    }
    let els: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("mutableListOf({})", els.join(", "))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("\"");
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(&escape_str(value)),
            InterpPart::Expr { expr } => {
                out.push_str(&format!("${{{}}}", emit_expr(expr)));
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
        // nil-coalescing idiom maps to Kotlin's `?:` when the result
        // isn't a Bool.
        BoolOpKind::Or => {
            if matches!(e.ty.as_ref(), Some(crate::ty::Ty::Bool)) {
                format!("{l} || {r}")
            } else {
                format!("{l} ?: {r}")
            }
        }
    }
}

fn emit_if(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    let c = emit_expr(cond);
    // Properties the condition proves non-null are read with `!!` in the
    // branch where they hold (Kotlin won't smart-cast a mutable property).
    let (mut then_nn, mut else_nn) = (Vec::new(), Vec::new());
    guarded_nonnull(cond, &mut then_nn, &mut else_nn);
    let then = with_nonnull(&then_nn, || indent(&emit_expr(then_branch)));
    if is_empty_branch(else_branch) {
        format!("if ({c}) {{\n{then}\n}}")
    } else {
        let els = with_nonnull(&else_nn, || indent(&emit_expr(else_branch)));
        format!("if ({c}) {{\n{then}\n}} else {{\n{els}\n}}")
    }
}

/// Run `f` with `props` added to `NONNULL_PROPS` (restoring afterward), so
/// reads of those properties inside `f` get a `!!`.
fn with_nonnull<F: FnOnce() -> String>(props: &[String], f: F) -> String {
    let added: Vec<String> = NONNULL_PROPS.with(|p| {
        let mut set = p.borrow_mut();
        props.iter().filter(|n| set.insert((*n).clone())).cloned().collect()
    });
    let out = f();
    NONNULL_PROPS.with(|p| {
        let mut set = p.borrow_mut();
        for n in &added {
            set.remove(n);
        }
    });
    out
}

fn emit_case(scrutinee: &Expr, arms: &[Arm]) -> String {
    let s = emit_expr(scrutinee);
    let mut lines = Vec::new();
    let mut has_else = false;
    for arm in arms {
        let body = emit_expr(&arm.body);
        let body_block = if body.contains('\n') {
            format!("{{\n{}\n}}", indent(&body))
        } else {
            body
        };
        match &arm.pattern {
            Pattern::Wildcard | Pattern::Bind { .. } => {
                has_else = true;
                lines.push(format!("    else -> {body_block}"));
            }
            Pattern::Lit { value } => {
                lines.push(format!("    {} -> {body_block}", emit_literal(value)));
            }
            other => {
                lines.push(format!("    /* TODO pattern {other:?} */ else -> {body_block}"));
                has_else = true;
            }
        }
    }
    if !has_else {
        lines.push("    else -> null".to_string());
    }
    format!("when ({s}) {{\n{}\n}}", lines.join("\n"))
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let val = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let n = camel(name.as_str());
            let already = DECLARED.with(|d| d.borrow().contains(&n));
            if already {
                format!("{n} = {val}")
            } else {
                let is_var = REASSIGNED.with(|r| r.borrow().contains(&n));
                DECLARED.with(|d| {
                    d.borrow_mut().insert(n.clone());
                });
                let kw = if is_var { "var" } else { "val" };
                // Empty container with an inferred element type → annotate
                // the declaration and let the bare ctor adopt it.
                let empty_hash = matches!(&*value.node, ExprNode::Hash { entries, .. } if entries.is_empty());
                let empty_arr = matches!(&*value.node, ExprNode::Array { elements, .. } if elements.is_empty());
                if empty_hash || empty_arr {
                    if let Some(t) = CONTAINER_TYPES.with(|c| c.borrow().get(&n).cloned()) {
                        let ctor = if empty_hash { "mutableMapOf()" } else { "mutableListOf()" };
                        return format!("{kw} {n}: {t} = {ctor}");
                    }
                }
                // `var x = null` infers `Nothing?`; annotate from a later
                // non-nil assignment when we have one.
                let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
                if is_nil {
                    if let Some(ty) = NIL_TYPES.with(|t| t.borrow().get(&n).cloned()) {
                        return format!("{kw} {n}: {ty} = {val}");
                    }
                    return format!("{kw} {n}: Any? = {val}");
                }
                format!("{kw} {n} = {val}")
            }
        }
        // Whole-reassign of a thread-local singleton field (`@slots = {}`) →
        // `slots.set(...)` (sets the current thread's value).
        LValue::Ivar { name } if is_object_tl_field(&camel(name.as_str())) => {
            format!("{}.set({val})", camel(name.as_str()))
        }
        _ => format!("{} = {val}", lvalue_ref(target)),
    }
}

/// Reference form of an LValue (no declaration), shared by assignment and
/// compound-assignment. Ivar writes are `this.`-qualified so they work
/// inside `init` blocks where a constructor param shadows the property.
fn lvalue_ref(target: &LValue) -> String {
    match target {
        LValue::Var { name, .. } => camel(name.as_str()),
        LValue::Ivar { name } => format!("this.{}", camel(name.as_str())),
        LValue::Attr { recv, name } => format!("{}.{}", emit_expr(recv), camel(name.as_str())),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
        LValue::Const { path } => path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("."),
    }
}

fn emit_op_assign(target: &LValue, op: OpAssignOp, value: &Expr) -> String {
    let lhs = lvalue_ref(target);
    let v = emit_expr(value);
    match op {
        OpAssignOp::OrOr => format!("{lhs} = {lhs} ?: {v}"),
        OpAssignOp::AndAnd => format!("if ({lhs} != null) {{ {lhs} = {v} }}"),
        OpAssignOp::Add => format!("{lhs} += {v}"),
        OpAssignOp::Sub => format!("{lhs} -= {v}"),
        OpAssignOp::Mul => format!("{lhs} *= {v}"),
        OpAssignOp::Div => format!("{lhs} /= {v}"),
        OpAssignOp::Mod => format!("{lhs} %= {v}"),
        _ => format!("{lhs} = {lhs} /* TODO op-assign */ {v}"),
    }
}

/// Call-site nullability coercion for the nullable model finders: the
/// synthesized companion `last()`/`findBy()` return `T?`, but the IR
/// stamps a call like `Article.last` with the non-nil model type when
/// the typed registry proves the context expects an instance (member
/// chains: `Article.last().title`). Append `!!` so the Kotlin surface
/// matches the stamp — the Ruby contract is "nil only when the table is
/// empty", and a wrong assumption fails the test with an NPE rather
/// than a compile error, same as Ruby's NoMethodError on nil.
fn coerce_nullable_finder(
    rendered: String,
    recv: Option<&Expr>,
    method: &str,
    result_ty: Option<&crate::ty::Ty>,
) -> String {
    use crate::ty::Ty;
    if !matches!(method, "last" | "find_by") {
        return rendered;
    }
    let Some(r) = recv else { return rendered };
    if !matches!(&*r.node, ExprNode::Const { .. }) {
        return rendered;
    }
    // Class stamp, the Union{Class,Nil} the registry stamps on
    // `Article.last`, or — when the stamp is missing entirely — the
    // RECEIVER being a registered class (the swift emitter's
    // receiver-class fallback: Ruby's contract for a model finder is
    // `-> instance`; a nil traps either way, matching Ruby's
    // NoMethodError-on-nil at the member chain).
    let is_model_stamp = match result_ty {
        Some(Ty::Class { .. }) => true,
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
            matches!(non_nil.as_slice(), [Ty::Class { .. }])
        }
        None | Some(Ty::Untyped) | Some(Ty::Var { .. }) => {
            let ExprNode::Const { path } = &*r.node else { return rendered };
            let cls = type_name(
                &path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"),
            );
            CLASS_HIERARCHY.with(|h| h.borrow().contains_key(&cls))
        }
        Some(_) => false,
    };
    if is_model_stamp {
        return format!("{rendered}!!");
    }
    rendered
}

fn emit_raise(value: &Expr) -> String {
    match &*value.node {
        ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => {
            format!("throw RuntimeException({})", emit_expr(value))
        }
        _ => format!("throw {}", emit_expr(value)),
    }
}

/// Kotlin's `as` is a checked reference cast — it does NOT convert
/// between numeric types or stringify. The lowerer inserts `Cast` at
/// untyped-row boundaries to mean "coerce to this column type", so map
/// numeric/string targets to the conversion functions; reference targets
/// keep `as`.
/// True when `arg` is already the target scalar type, so a `self.<col> =`
/// coercion would be redundant: either the IR already typed it (the value
/// carries `target_ty`) or it's an explicit `Cast` to that type (the
/// `from_row` shape). Guards against double-coercion.
fn arg_already_ty(arg: &Expr, target_ty: &crate::ty::Ty) -> bool {
    if let ExprNode::Cast { target_ty: t, .. } = &*arg.node {
        return t == target_ty;
    }
    arg.ty.as_ref() == Some(target_ty)
}

fn emit_cast(value: &Expr, target_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let v = emit_expr(value);
    match target_ty {
        Ty::Int => format!("({v}).toString().toLong()"),
        Ty::Float => format!("({v}).toString().toDouble()"),
        Ty::Str | Ty::Sym => format!("({v}).toString()"),
        _ => format!("({v} as {})", kotlin_ty(target_ty)),
    }
}

/// `recv[begin..]` / `recv[begin..end]` → Kotlin `substring`. Indices are
/// `Long` (Ty::Int → Long), so `.toInt()` for the String API.
fn emit_slice_range(
    rs: &str,
    begin: Option<&Expr>,
    end: Option<&Expr>,
    exclusive: bool,
) -> String {
    let b = begin.map(emit_expr).unwrap_or_else(|| "0L".to_string());
    match end {
        None => format!("{rs}.substring(({b}).toInt())"),
        Some(e) => {
            let e = emit_expr(e);
            let end_idx = if exclusive {
                format!("({e}).toInt()")
            } else {
                format!("(({e}) + 1).toInt()")
            };
            format!("{rs}.substring(({b}).toInt(), {end_idx})")
        }
    }
}

fn emit_lambda(params: &[crate::ident::Symbol], body: &Expr, destructure: bool) -> String {
    let body_s = emit_expr(body);
    if params.is_empty() {
        format!("{{ {body_s} }}")
    } else {
        let ps: Vec<String> = params.iter().map(|p| camel(p.as_str())).collect();
        // Kotlin `Map.forEach` yields a single `Map.Entry`; destructure it.
        let head = if destructure {
            format!("({})", ps.join(", "))
        } else {
            ps.join(", ")
        };
        format!("{{ {head} -> {body_s} }}")
    }
}

/// Methods that look like 0-arg attribute reads but are real method calls
/// (need `()` in Kotlin). Everything else with a receiver and no args is
/// emitted as property access.
fn forces_parens(method: &str) -> bool {
    matches!(
        method,
        "save" | "save!" | "destroy" | "destroy!" | "reload" | "validate" | "dup" | "clone"
    )
}

/// Test a receiver's type with `pred`, looking through a nullable
/// `Union{T, Nil}` to the underlying `T`. A `pins: Array[…]?` receiver still
/// dispatches as an Array (the call site has already null-guarded it).
fn ty_is(ty: Option<&crate::ty::Ty>, pred: impl Fn(&crate::ty::Ty) -> bool) -> bool {
    match ty {
        Some(crate::ty::Ty::Union { variants }) => variants
            .iter()
            .filter(|t| !matches!(t, crate::ty::Ty::Nil))
            .any(|t| pred(t)),
        Some(t) => pred(t),
        None => false,
    }
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
) -> String {
    // A bare implicit-self zero-arg send that names a parameter is a
    // reference to that local, not a method call — emit the identifier
    // without `()`. (See `PARAM_NAMES`: the view lowerer renders a partial
    // local in argument position as a `Send`.)
    if recv.is_none() && args.is_empty() && block.is_none() && is_param(method) {
        return camel(method);
    }

    // Ruby logical-not `!x` lowers to a no-receiver `!` send with one arg
    // (e.g. `any?`/`present?` normalize to `! …empty?` / `! …nil?`).
    if recv.is_none() && method == "!" && args.len() == 1 {
        return format!("!({})", emit_expr(&args[0]));
    }

    let args_s: Vec<String> = args.iter().map(emit_expr).collect();

    // `self.class.METHOD(...)` → unqualified `METHOD(...)`. Ruby's
    // `self.class` reflection has no Kotlin analog, but a per-model class
    // method lives on the companion, and Kotlin lets an instance method
    // call companion members by simple name. (`self.class.schema_columns`
    // in `fill_timestamps` → `schemaColumns()`.)
    if let Some(r) = recv {
        if let ExprNode::Send { recv: Some(inner), method: m2, args: a2, .. } = &*r.node {
            if m2.as_str() == "class"
                && a2.is_empty()
                && matches!(&*inner.node, ExprNode::SelfRef)
            {
                return format!("{}({})", camel(method), args_s.join(", "));
            }
        }
    }

    // Stdlib module calls with no roundhouse runtime object: map directly
    // to the JVM. `Base64.strict_encode64(x)` → java.util.Base64;
    // `JSON.generate(x)` → the transpiled `JsonBuilder.encodeValue`.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            match (path.last().map(|s| s.as_str()), method) {
                (Some("Base64"), "strict_encode64") => {
                    return format!(
                        "java.util.Base64.getEncoder().encodeToString(({}).toByteArray())",
                        args_s[0]
                    );
                }
                (Some("JSON"), "generate") => {
                    return format!("JsonBuilder.encodeValue({})", args_s[0]);
                }
                // Temporal reader intrinsic: `ActiveSupport.parse_db_time(s)`
                // parses stored ISO-8601 text into a native `OffsetDateTime`.
                // Renders to the hand-written `RhDateTime.parse` (nil-safe:
                // `String?` → `OffsetDateTime?`). The argument is the column's
                // `String` storage backing (an ivar that `emit_node` already
                // redirects to `<name>Raw`).
                (Some("ActiveSupport"), "parse_db_time") if args.len() == 1 => {
                    return format!("RhDateTime.parse({})", args_s[0]);
                }
                // Temporal writer intrinsic: `ActiveSupport.db_now` — current
                // UTC time in Rails' exact storage form ("YYYY-MM-DD
                // HH:MM:SS.ffffff"). `fill_timestamps` stamps with it so a
                // column's TEXT values stay homogeneous — and
                // lexicographically ordered — when a roundhouse-emitted app
                // shares a database with a real Rails app.
                (Some("ActiveSupport"), "db_now") if args.is_empty() => {
                    return "RhDateTime.dbNow()".to_string();
                }
                _ => {}
            }
        }
    }

    // Constructor: `X.new(...)` → `X(...)`. Implicit-self `new(...)` (a
    // companion factory) resolves to the current class's constructor. But a
    // view module with an actual `new` method (the `new.html.erb` render,
    // `Views::Articles.new`) is a method call — keep it, backtick-escaping
    // the keyword.
    if method == "new" {
        let is_method = method_params_for(recv, "new").is_some();
        if let Some(r) = recv {
            if is_method {
                return format!("{}.`new`({})", emit_expr(r), emit_call_args(recv, "new", args));
            }
            return format!("{}({})", emit_expr(r), args_s.join(", "));
        }
        let cls = CURRENT_CLASS.with(|c| c.borrow().clone());
        if !cls.is_empty() {
            return format!("{cls}({})", args_s.join(", "));
        }
    }

    // `raise Class, msg` → `throw Class(msg)`; `raise Class` → `throw
    // Class()`; `raise Class, obj` → `throw Class(obj)`. The exception
    // classes (`NotImplementedError` is Kotlin stdlib; `RecordNotFound` /
    // `RecordInvalid` live in `Errors.kt`) take the message/record as a
    // constructor arg. Bare `raise "str"` arrives as a `Raise` node and is
    // handled by `emit_raise`.
    if method == "raise" && recv.is_none() && !args.is_empty() {
        if let ExprNode::Const { path } = &*args[0].node {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            let cls = if joined.is_empty() { "RuntimeException".to_string() } else { type_name(&joined) };
            return format!("throw {cls}({})", args_s[1..].join(", "));
        }
        return format!("throw RuntimeException({})", args_s.join(", "));
    }

    // Attribute setter: `recv.foo = v` arrives as a Send named `foo=`.
    if let (Some(r), 1) = (recv, args.len()) {
        if method.ends_with('=') && !matches!(method, "==" | "!=" | "<=" | ">=") {
            let base = &method[..method.len() - 1];
            // `self.<col> = <untyped>` (assign_from_row / initialize /
            // update read `row[k]`/`attrs[k]` as `Any?`) — coerce to the
            // column's scalar type. Only for a `self` receiver: `from_row`
            // writes to an `instance.` local and already carries the Cast,
            // and other-receiver setters target a different class's props.
            if matches!(&*r.node, ExprNode::SelfRef) {
                if let Some(ty) = instance_prop_scalar_ty(base) {
                    if !arg_already_ty(&args[0], &ty) {
                        return format!("this.{} = {}", camel(base), emit_cast(&args[0], &ty));
                    }
                }
            }
            return format!("{}.{} = {}", emit_expr(r), camel(base), args_s[0]);
        }
    }

    // `is_a?(Class)` → Kotlin `is` / boolean compare.
    if method == "is_a?" && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            let rs = emit_expr(r);
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            return match last {
                "TrueClass" => format!("({rs} == true)"),
                "FalseClass" => format!("({rs} == false)"),
                "Integer" => format!("{rs} is Long"),
                "Float" => format!("{rs} is Double"),
                "String" => format!("{rs} is String"),
                "Numeric" => format!("{rs} is Number"),
                "Hash" => format!("{rs} is Map<*, *>"),
                "Array" => format!("{rs} is List<*>"),
                other => format!("{rs} is {}", type_name(other)),
            };
        }
    }

    // `recv.gsub(pattern, hash)` → regex replace with a map lookup.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "{}.replace({}) {{ (({})[it.value] ?: it.value).toString() }}",
                args_s[0],
                emit_expr(r),
                args_s[1]
            );
        }
    }

    // String predicates with one arg.
    if let (Some(r), 1) = (recv, args.len()) {
        match method {
            "start_with?" => return format!("{}.startsWith({})", emit_expr(r), args_s[0]),
            "end_with?" => return format!("{}.endsWith({})", emit_expr(r), args_s[0]),
            "include?" => return format!("{}.contains({})", emit_expr(r), args_s[0]),
            "join" => return format!("{}.joinToString({})", emit_expr(r), args_s[0]),
            _ => {}
        }
    }

    // Indexing / slicing.
    if method == "[]" {
        if let Some(r) = recv {
            let rs = emit_expr(r);
            if args.len() == 1 {
                // `str[a..]` / `str[a..b]` slice.
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    return emit_slice_range(&rs, begin.as_ref(), end.as_ref(), *exclusive);
                }
                // List/Array index needs an Int (indices are `Long`).
                if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Array { .. })) {
                    return format!("{rs}[({}).toInt()]", args_s[0]);
                }
                return format!("{rs}[{}]", args_s[0]);
            }
            if args.len() == 2 {
                // Ruby `str[start, len]` → `substring(start, start + len)`.
                let start = &args_s[0];
                let len = &args_s[1];
                return format!(
                    "{rs}.substring(({start}).toInt(), (({start}) + ({len})).toInt())"
                );
            }
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
        // `<<` / `push` → MutableList.add.
        if method == "<<" || method == "push" {
            return format!("{}.add({})", emit_expr(r), args_s[0]);
        }
        // Hash key test — only on an actual Map receiver. On a user type
        // (Flash's `has_key?` delegating to its own `key?`) it stays a call.
        if (method == "key?" || method == "has_key?") && recv_is_hash(r) {
            return format!("{}.containsKey({})", emit_expr(r), args_s[0]);
        }
        // `Hash#delete(k)` → `MutableMap.remove(k)` — only on an actual
        // Map receiver (same gate as `key?`): the controller-test
        // `delete(path)` HTTP helper is a real method on the inherited
        // RoundhouseTestCase and must stay a call.
        if method == "delete" && recv_is_hash(r) {
            return format!("{}.remove({})", emit_expr(r), args_s[0]);
        }
        // `Hash#merge(other)` → a new map (`+` yields a read-only Map; the
        // runtime treats it mutably, so re-wrap). Used by the form/link
        // helpers (`{href: …}.merge(opts)`).
        if method == "merge" {
            return format!("({} + {}).toMutableMap()", emit_expr(r), args_s[0]);
        }
    }
    if let (Some(r), 2) = (recv, args.len()) {
        if method == "[]=" {
            return format!("{}[{}] = {}", emit_expr(r), args_s[0], args_s[1]);
        }
        // `Hash#fetch(k, default)` → `(recv[k] ?: default)` (Ruby returns
        // the value or the default; Kotlin map-get is null for missing).
        if method == "fetch" {
            return format!("({}[{}] ?: {})", emit_expr(r), args_s[0], args_s[1]);
        }
        // `String#tr(from, to)` → `replace` (single-char translation, the
        // only shape the runtime uses: `key.tr("_", "-")`).
        if method == "tr" {
            return format!("{}.replace({}, {})", emit_expr(r), args_s[0], args_s[1]);
        }
    }

    // Zero-arg receiver sends: builtin coercions, then property vs method.
    if let (Some(r), true) = (recv, args.is_empty() && block.is_none()) {
        let rs = emit_expr(r);
        match method {
            "nil?" => return format!("({rs} == null)"),
            "!" => return format!("!({rs})"),
            "to_s" => return format!("{rs}.toString()"),
            "to_i" => return format!("{rs}.toString().toLong()"),
            "to_f" => return format!("{rs}.toString().toDouble()"),
            "empty?" => return format!("{rs}.isEmpty()"),
            "any?" => return format!("{rs}.isNotEmpty()"),
            "upcase" => return format!("{rs}.uppercase()"),
            "downcase" => return format!("{rs}.lowercase()"),
            "strip" => return format!("{rs}.trim()"),
            // `Array#join` with no separator → `joinToString("")`.
            "join" => return format!("{rs}.joinToString(\"\")"),
            // `.length`/`.size`: collections use `.size`, strings `.length`.
            // Both are Kotlin `Int`; `.toLong()` keeps them in the
            // Long-everywhere world (Ruby Integer → Long) so `==` against a
            // Long literal works (`<`/`>` already cross Int/Long, but `==`
            // does not).
            "length" | "size" => {
                return if recv_is_array(r) || recv_is_hash(r) {
                    format!("{rs}.size.toLong()")
                } else {
                    format!("{rs}.length.toLong()")
                };
            }
            // `count` with no args on a collection is `size` (Kotlin's
            // `.count()` extension also works but `.size` avoids a call).
            "count" if recv_is_array(r) || recv_is_hash(r) => {
                return format!("{rs}.size.toLong()");
            }
            // Ruby `Hash#keys`/`#values` return Arrays (ordered, indexable);
            // Kotlin's are a Set/Collection, so materialize a MutableList.
            "keys" if recv_is_hash(r) => return format!("{rs}.keys.toMutableList()"),
            "values" if recv_is_hash(r) => return format!("{rs}.values.toMutableList()"),
            // No-ops in Kotlin — drop, keep the receiver.
            "freeze" | "dup" | "to_a" => return rs,
            // `to_h` is a no-op on a Hash; on a user type (e.g. `Session`,
            // `Flash`) it's a real `toH()` method — fall through to the call.
            "to_h" if recv_is_hash(r) => return rs,
            _ => {}
        }
        // A `Const` receiver (a class / object like `Db`, `Broadcasts`)
        // means a 0-arg *method* call — unless it names a module/object
        // accessor property (`ActiveRecord.adapter`), which reads as a
        // Kotlin property.
        if matches!(&*r.node, ExprNode::Const { .. }) {
            if is_object_prop(&rs, method) {
                return format!("{rs}.{}", camel(method));
            }
            return format!("{rs}.{}()", camel(method));
        }
        // A `self` receiver: a 0-arg send is a Kotlin method call (`()`)
        // unless it names an accessor-backed property of this class. In
        // Ruby every `self.x` is a method call; the only ones that became
        // Kotlin properties are the `attr_*` / body-ivar fields. This is
        // why `self.before_validation` / `self._adapter_insert` /
        // `self.table_name` (companion) must keep their parens.
        if matches!(&*r.node, ExprNode::SelfRef) {
            return if is_instance_prop(method) {
                format!("{rs}.{}", camel(method))
            } else {
                format!("{rs}.{}()", camel(method))
            };
        }
        // A typed-`Class` receiver whose member is a known zero-arg
        // instance *method* (not a column/accessor property) keeps its
        // `()` — `article.comments` (has-many loader) → `article.comments()`.
        if let Some(cls) = receiver_class_name(r) {
            if is_instance_method_of(&cls, method) {
                return format!("{rs}.{}()", camel(method));
            }
        }
        if !forces_parens(method) && !method.ends_with('?') && !method.ends_with('!') {
            // Attribute read on a non-self instance receiver (its concrete
            // property set isn't known here; default to the read form).
            return format!("{rs}.{}", camel(method));
        }
    }

    // Block → Kotlin trailing lambda. `.each` maps to `.forEach` on
    // Kotlin collections (List: 1 param; Map: destructured `(k, v)`); on
    // a user type (e.g. Flash/Session, whose `each` takes a block param)
    // it stays `each`.
    if let Some(b) = block {
        let recv_arr = recv.is_some_and(|r| ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Array { .. })));
        let recv_hash = recv.is_some_and(|r| ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Hash { .. })));
        let kt_method = if method == "each" && (recv_arr || recv_hash) {
            "forEach".to_string()
        } else {
            camel(method)
        };
        let lam = match &*b.node {
            ExprNode::Lambda { params, body, .. } => emit_lambda(params, body, recv_hash),
            _ => emit_expr(b),
        };
        let base = match recv {
            Some(r) => format!("{}.{kt_method}", emit_expr(r)),
            None => kt_method,
        };
        // Kotlin `.map` yields a read-only `List`; roundhouse models arrays
        // as `MutableList`, so coerce back to match declared types.
        let tail = if method == "map" { ".toMutableList()" } else { "" };
        if args_s.is_empty() {
            return format!("{base} {lam}{tail}");
        }
        return format!("{base}({}) {lam}{tail}", args_s.join(", "));
    }

    // General call. A trailing `kwargs: true` hash splats into Kotlin named
    // arguments (`truncate(body, length = 100)`) when the callee is known to
    // have matching named params; otherwise it stays a map literal.
    let name = camel(method);
    let call_args = emit_call_args(recv, method, args);
    match recv {
        Some(r) => format!("{}.{name}({call_args})", emit_expr(r)),
        None => format!("{name}({call_args})"),
    }
}

/// Render a call's argument list, splatting a trailing keyword-args hash
/// (`Hash { kwargs: true }`) into Kotlin named arguments — `key = value` per
/// entry, the `key` camelCased to match the parameter. Only splats when the
/// callee (`receiver.method`) is registered and the keys are a subset of its
/// params; otherwise (Map-param callees like `Broadcasts.append`, or
/// unregistered receivers) the hash stays a map literal. The `kwargs` flag
/// is set by ingest, so it never misfires on a sym-keyed map arg.
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
                if let Some(keys) = keys {
                    if kwargs_match_params(recv, method, &keys) {
                        let mut parts: Vec<String> = head.iter().map(emit_expr).collect();
                        for (k, (_, v)) in keys.iter().zip(entries.iter()) {
                            parts.push(format!("{k} = {}", emit_expr(v)));
                        }
                        return parts.join(", ");
                    }
                }
            }
        }
    }
    args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
}
