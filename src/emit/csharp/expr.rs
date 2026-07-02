//! `Expr` → C# source.
//!
//! Ported from `src/emit/kotlin/expr.rs` (C# and Kotlin share a profile:
//! nominal, GC'd, declared nullability). The bookkeeping layer — the
//! per-method local/hoist/container scans and the class/accessor registries
//! — is the same; the rendering layer diverges where C# forces choices
//! Kotlin doesn't:
//!   - **Statements need `;`** and C# splits statements from expressions, so
//!     rendering is split into `emit_stmt` (body lines) and `emit_expr`
//!     (value positions). Control flow renders as a *block* in statement
//!     position (`if (c) { … }`) and as a *ternary / switch-expression* in
//!     value position.
//!   - `?:` (Kotlin Elvis) → `??`; `!!` (not-null assert) → `!`
//!     (null-forgiving); string templates `"${x}"` → `$"{x}"`; `is X` casts
//!     and collection/map literals take their C# spellings.
//!
//! Member identifiers (methods + public properties) emit idiomatic PascalCase
//! at both definitions and references via `naming::pascal`; locals, parameters,
//! block params, and StringBuilder buffers stay camelCase (`naming::camel`).
//! The internal classification maps remain keyed by `camel(rubyname)` — a
//! canonical normalization, not emitted output — so the lookup/emit split lives
//! at each site. Untyped edge nodes emit a `/* TODO kind */` marker rather than
//! panicking.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::expr::{
    Arm, BoolOpKind, Expr, ExprNode, InterpPart, IrHint, LValue, Literal, OpAssignOp, Pattern,
};

use super::naming::{camel, pascal, pascal_of_camel, type_name};
use super::ty::csharp_ty;

thread_local! {
    /// Local names already declared in the current method body (so the
    /// first `Assign` emits a declaration and later ones a bare `=`).
    static DECLARED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// For locals first assigned `nil`, the nullable C# type taken from a
    /// later non-nil assignment — so `var x = null` (illegal in C#) becomes
    /// `T? x = null`.
    static NIL_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// For locals first assigned an empty `{}`/`[]`, the C# container type
    /// inferred from later `map[k]=v` / `list << x` — so the empty literal
    /// gets a precise `new List<T>()` instead of `object?`.
    static CONTAINER_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Whether the method currently being emitted returns `void`. A guard
    /// `return nil` in a void method emits a bare `return;`.
    static RETURNS_UNIT: RefCell<bool> = const { RefCell::new(false) };
    /// camelCased names of the current class's accessor-backed properties
    /// (`attr_*` + body ivars). A zero-arg `self`-receiver send resolves to
    /// a property read only when its name is in here.
    static INSTANCE_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// camelCased parameter names of the method currently being emitted.
    static PARAM_NAMES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Instance property name → declared `Ty`, so a `self.col = <object?>`
    /// write (the row/attrs column shape) can coerce the value to the
    /// column's scalar type.
    static INSTANCE_PROP_TYPES: RefCell<HashMap<String, crate::ty::Ty>> =
        RefCell::new(HashMap::new());
    /// `"Object.prop"` keys for module/object-level accessor properties
    /// (`class << self; attr_accessor :adapter` → `ActiveRecord.adapter`).
    static OBJECT_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Name of the class currently being emitted, so an implicit-self
    /// `new(attrs)` resolves to the C# constructor `new Base(attrs)`.
    static CURRENT_CLASS: RefCell<String> = const { RefCell::new(String::new()) };
    /// Class hierarchy: simple class name → (parent simple name, instance
    /// member names). For override resolution.
    static CLASS_HIERARCHY: RefCell<HashMap<String, (Option<String>, HashSet<String>)>> =
        RefCell::new(HashMap::new());
    /// Class simple name → camelCased names of its zero-arg instance methods
    /// (excludes property accessors). A zero-arg send to a typed-`Class`
    /// receiver whose member is in this set keeps its `()`.
    static CLASS_INSTANCE_METHODS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// `"Receiver.method"` → the callee's camelCased parameter names. Decides
    /// whether a call-site `kwargs:true` hash splats into named arguments.
    static METHOD_PARAMS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
    /// Full `T name = default;` declarations for locals first assigned inside
    /// a nested scope yet used at an outer level — they hoist to the method
    /// top (emitted by `library::emit_method`).
    static HOISTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// camelCased instance-property names proven non-null by an enclosing
    /// `if (!prop.nil?)` guard — read with `!` (null-forgiving).
    static NONNULL_PROPS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// camelCased `@ivar` names of the current object/module that hold mutable
    /// singleton state — emitted as a thread-local (`name.Value`).
    static OBJECT_TL_FIELDS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// `@ivar` (camelCased) → the C# field name to read/write it as, when the
    /// ivar's natural name collides with a same-named method (C# forbids a
    /// property and method sharing a name — `base.rb`'s `@errors` + `errors`).
    /// The colliding ivar emits as a private renamed field.
    static IVAR_RENAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

pub(super) fn set_ivar_renames(m: HashMap<String, String>) {
    IVAR_RENAMES.with(|r| *r.borrow_mut() = m);
}

thread_local! {
    /// True while emitting a `static` (class) method body — `self` there is
    /// the class, not an instance, so `self.x()` must render as `Class.x()`
    /// (C# forbids `this` in a static member).
    static IN_STATIC: RefCell<bool> = const { RefCell::new(false) };
}

pub(super) fn set_in_static(b: bool) {
    IN_STATIC.with(|s| *s.borrow_mut() = b);
}

fn in_static() -> bool {
    IN_STATIC.with(|s| *s.borrow())
}

thread_local! {
    /// Per-method counter for unique `foreach` entry-var names (nested
    /// hash-each would otherwise collide on `__kv`). Reset in `begin_method`.
    static LOOP_ID: RefCell<usize> = const { RefCell::new(0) };
}

fn next_loop_id() -> usize {
    LOOP_ID.with(|c| {
        let mut b = c.borrow_mut();
        *b += 1;
        *b
    })
}

/// Map a Ruby stdlib exception class to its C# analog (app/runtime exception
/// classes like `RecordNotFound` pass through unchanged).
fn map_exception_class(name: &str) -> String {
    match name {
        "NotImplementedError" => "NotImplementedException".to_string(),
        "ArgumentError" => "ArgumentException".to_string(),
        "RuntimeError" | "StandardError" => "Exception".to_string(),
        "TypeError" => "InvalidCastException".to_string(),
        "KeyError" | "IndexError" => "KeyNotFoundException".to_string(),
        other => type_name(other),
    }
}

/// The C# member name an `@ivar` reads/writes as. A registered collision rename
/// (`@errors` → `_errors`) keeps its private camelCase backing field; otherwise
/// the ivar is an accessor-backed public property and emits PascalCase. The
/// rename lookup is keyed by the canonical `camel` name.
fn ivar_name(name: &str) -> String {
    let c = camel(name);
    IVAR_RENAMES.with(|r| r.borrow().get(&c).cloned()).unwrap_or_else(|| pascal(name))
}

pub(super) fn set_object_tl_fields(names: HashSet<String>) {
    OBJECT_TL_FIELDS.with(|f| *f.borrow_mut() = names);
}

fn is_object_tl_field(name: &str) -> bool {
    OBJECT_TL_FIELDS.with(|f| f.borrow().contains(name))
}

/// Append `!` (null-forgiving) to a property read proven non-null by a guard.
fn nonnull_read(name: String) -> String {
    if NONNULL_PROPS.with(|p| p.borrow().contains(&name)) {
        format!("{name}!")
    } else {
        name
    }
}

fn read_prop_name(e: &Expr) -> Option<String> {
    // `raw` keys the `is_instance_prop` lookup (which canonicalizes via `camel`);
    // `emitted` is the identifier the corresponding read renders to — a property
    // ivar/self-send emits PascalCase, a bare local stays camelCase — so a
    // proven-non-null entry in `NONNULL_PROPS` matches its read site.
    let (raw, emitted): (&str, String) = match &*e.node {
        ExprNode::Ivar { name } => (name.as_str(), pascal(name.as_str())),
        ExprNode::Var { name, .. } => (name.as_str(), camel(name.as_str())),
        ExprNode::Send { recv, method, args, .. }
            if args.is_empty()
                && matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef)) =>
        {
            (method.as_str(), pascal(method.as_str()))
        }
        _ => return None,
    };
    is_instance_prop(raw).then_some(emitted)
}

fn nil_test_prop(e: &Expr) -> Option<String> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if method.as_str() == "nil?" && args.is_empty() {
            return read_prop_name(r);
        }
    }
    None
}

fn guarded_nonnull(cond: &Expr, then_nn: &mut Vec<String>, else_nn: &mut Vec<String>) {
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

pub(super) fn hoisted_decls() -> Vec<String> {
    HOISTED.with(|h| h.borrow().clone())
}

pub(super) fn reset_method_params() {
    METHOD_PARAMS.with(|m| m.borrow_mut().clear());
}

pub(super) fn register_method_params(receiver: &str, method: &str, params: HashSet<String>) {
    METHOD_PARAMS.with(|m| {
        m.borrow_mut().insert(format!("{receiver}.{}", camel(method)), params);
    });
}

fn method_params_lookup(receiver: &str, method: &str) -> Option<HashSet<String>> {
    METHOD_PARAMS.with(|m| m.borrow().get(&format!("{receiver}.{}", camel(method))).cloned())
}

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

fn kwargs_match_params(recv: Option<&Expr>, method: &str, keys: &[String]) -> bool {
    method_params_for(recv, method).map(|p| keys.iter().all(|k| p.contains(k))).unwrap_or(false)
}

pub(super) fn register_instance_methods(name: &str, methods: HashSet<String>) {
    CLASS_INSTANCE_METHODS.with(|m| m.borrow_mut().insert(name.to_string(), methods));
}

thread_local! {
    /// Class simple name → its emitted *static* method names. A model static
    /// (`Article.find`) that matches a name an ancestor also defines as a
    /// static (`Base.find`) HIDES it in C# (statics don't override), which
    /// warns CS0108 unless marked `new`.
    static CLASS_STATIC_METHODS: RefCell<HashMap<String, HashSet<String>>> =
        RefCell::new(HashMap::new());
}

pub(super) fn register_static_methods(name: &str, methods: HashSet<String>) {
    CLASS_STATIC_METHODS.with(|m| m.borrow_mut().insert(name.to_string(), methods));
}

/// The static method names visible from `class_name`'s *ancestors* (walking
/// parents, excluding the class itself) — the set a static must mark `new` to
/// shadow without a warning.
pub(super) fn ancestor_static_methods(class_name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = CLASS_HIERARCHY
        .with(|h| h.borrow().get(class_name).and_then(|(p, _)| p.clone()));
    let mut guard = 0;
    while let Some(name) = cur {
        guard += 1;
        if guard > 32 {
            break;
        }
        if let Some(set) = CLASS_STATIC_METHODS.with(|m| m.borrow().get(&name).cloned()) {
            out.extend(set);
        }
        cur = CLASS_HIERARCHY.with(|h| h.borrow().get(&name).and_then(|(p, _)| p.clone()));
    }
    out
}

fn instance_prop_ty(name: &str) -> Option<crate::ty::Ty> {
    INSTANCE_PROP_TYPES.with(|t| t.borrow().get(&camel(name)).cloned())
}

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

fn receiver_class_name(r: &Expr) -> Option<String> {
    match r.ty.as_ref()? {
        crate::ty::Ty::Class { id, .. } => Some(type_name(id.0.as_str())),
        _ => None,
    }
}

pub(super) fn reset_class_hierarchy() {
    CLASS_HIERARCHY.with(|h| h.borrow_mut().clear());
}

pub(super) fn register_class_hierarchy(name: &str, parent: Option<&str>, members: HashSet<String>) {
    CLASS_HIERARCHY
        .with(|h| h.borrow_mut().insert(name.to_string(), (parent.map(str::to_string), members)));
}

pub(super) fn ancestor_members(class_name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = Some(class_name.to_string());
    let mut guard = 0;
    while let Some(name) = cur {
        guard += 1;
        if guard > 32 {
            break;
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
        let methods =
            CLASS_INSTANCE_METHODS.with(|m| m.borrow().get(&name).cloned().unwrap_or_default());
        out.extend(members.into_iter().filter(|m| !methods.contains(m)));
        cur = parent;
    }
    out
}

/// A class-name `Const` in value position whose PascalCase spelling collides
/// with an in-scope instance member is namespace-qualified (`Roundhouse.X`) so
/// it binds to the type rather than the shadowing member. PascalCasing members
/// reintroduces collisions camelCase kept apart — a `comment_params` method vs
/// the synthesized `CommentParams` class, or an `@articles : List<Article>` ivar
/// vs the `Articles` view module. C#'s "Color Color" rule already resolves the
/// case where the colliding member is a *value* (field/property) of the
/// identically-named type (an `@article : Article` ivar vs the `Article` type),
/// so those stay bare; a method, or a value of a different type, must be
/// qualified. The flat `Roundhouse` namespace makes the prefix unambiguous.
fn qualify_colliding_const(emitted: String) -> String {
    let cls = CURRENT_CLASS.with(|c| c.borrow().clone());
    if cls.is_empty() {
        return emitted;
    }
    let colliding: Vec<String> =
        ancestor_members(&cls).into_iter().filter(|cm| pascal_of_camel(cm) == emitted).collect();
    if colliding.is_empty() {
        return emitted;
    }
    let all_color_color = colliding.iter().all(|cm| {
        INSTANCE_PROP_TYPES
            .with(|t| t.borrow().get(cm).map(csharp_ty))
            .map(|cs| cs == emitted)
            .unwrap_or(false)
    });
    if all_color_color {
        emitted
    } else {
        format!("Roundhouse.{emitted}")
    }
}

pub(super) fn set_current_class(name: &str) {
    CURRENT_CLASS.with(|c| *c.borrow_mut() = name.to_string());
}

pub(super) fn reset_object_accessors() {
    OBJECT_PROPS.with(|p| p.borrow_mut().clear());
}

pub(super) fn register_object_accessor(object: &str, prop: &str) {
    OBJECT_PROPS.with(|p| p.borrow_mut().insert(format!("{object}.{}", camel(prop))));
}

fn is_object_prop(object: &str, method: &str) -> bool {
    OBJECT_PROPS.with(|p| p.borrow().contains(&format!("{object}.{}", camel(method))))
}

pub(super) fn set_instance_props(props: HashSet<String>) {
    INSTANCE_PROPS.with(|p| *p.borrow_mut() = props);
}

pub(super) fn set_instance_prop_types(types: HashMap<String, crate::ty::Ty>) {
    INSTANCE_PROP_TYPES.with(|t| *t.borrow_mut() = types);
}

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

pub(super) fn set_param_names(names: HashSet<String>) {
    PARAM_NAMES.with(|p| *p.borrow_mut() = names);
}

fn is_param(method: &str) -> bool {
    PARAM_NAMES.with(|p| p.borrow().contains(&camel(method)))
}

pub(super) fn set_returns_unit(b: bool) {
    RETURNS_UNIT.with(|r| *r.borrow_mut() = b);
}

/// Reset per-method local-decl tracking and pre-scan the body for the
/// container/nil/hoist signals. Called by `library::emit_method`.
pub(super) fn begin_method(body: &Expr) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut nil_types: HashMap<String, String> = HashMap::new();
    count_assigns(body, &mut counts, &mut nil_types);
    DECLARED.with(|d| d.borrow_mut().clear());
    LOOP_ID.with(|c| *c.borrow_mut() = 0);
    NIL_TYPES.with(|t| *t.borrow_mut() = nil_types);

    let mut container_types: HashMap<String, String> = HashMap::new();
    scan_container_types(body, &mut container_types);
    CONTAINER_TYPES.with(|t| *t.borrow_mut() = container_types);

    let hoist = scan_hoist(body);
    DECLARED.with(|d| {
        let mut set = d.borrow_mut();
        for (n, _) in &hoist {
            set.insert(n.clone());
        }
    });
    HOISTED.with(|h| {
        *h.borrow_mut() = hoist
            .iter()
            .map(|(n, ty)| {
                // Prefer the nullable nil-first type (a `result` assigned
                // `null` then `Article` should hoist as `Article?`, not
                // `object?`), so the eventual `return result` type-checks.
                let ty = NIL_TYPES
                    .with(|t| t.borrow().get(n).cloned())
                    .unwrap_or_else(|| ty.clone());
                format!("{ty} {n} = {};", cs_default(&ty))
            })
            .collect();
    });
}

/// The C# type an `is_a?(Class)` narrows to, for the smart-cast in
/// `emit_if_expr`. `None` for classes with no clean cast target (Hash/Array →
/// interfaces, the boolean/numeric pseudo-classes).
fn is_a_cast_type(last: &str) -> Option<String> {
    match last {
        "Integer" => Some("long".to_string()),
        "Float" => Some("double".to_string()),
        "String" => Some("string".to_string()),
        "TrueClass" | "FalseClass" | "Numeric" | "Hash" | "Array" => None,
        other => Some(type_name(other)),
    }
}

/// Default initializer for a C# type *name* (string form), for hoisted
/// declarations. Nullable types and unknowns default to `null`.
fn cs_default(ty: &str) -> String {
    match ty {
        "long" => "0L".to_string(),
        "int" => "0".to_string(),
        "double" => "0.0".to_string(),
        "bool" => "false".to_string(),
        "string" => "\"\"".to_string(),
        _ if ty.ends_with('?') => "null".to_string(),
        _ if ty.starts_with("List<") || ty.starts_with("Dictionary<") => format!("new {ty}()"),
        _ => "null".to_string(),
    }
}

fn scan_hoist(body: &Expr) -> Vec<(String, String)> {
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
            Some(t) if !matches!(t, crate::ty::Ty::Nil) => csharp_ty(t),
            _ => "object?".to_string(),
        };
        let entry = info.entry(n).or_insert((depth, 0, ty));
        entry.1 += 1;
    }
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

/// Infer C# container types for empty-container locals from how they're later
/// populated: `map[k] = v` → `Dictionary<K, V>`; `list << x` → `List<E>`.
fn scan_container_types(e: &Expr, out: &mut HashMap<String, String>) {
    let nn = |ty: Option<&crate::ty::Ty>| -> String {
        match ty {
            Some(crate::ty::Ty::Union { variants }) => {
                let nn: Vec<&crate::ty::Ty> =
                    variants.iter().filter(|t| !matches!(t, crate::ty::Ty::Nil)).collect();
                if nn.len() == 1 {
                    csharp_ty(nn[0])
                } else {
                    "object?".to_string()
                }
            }
            Some(t) => csharp_ty(t),
            None => "object?".to_string(),
        }
    };
    match &*e.node {
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => {
            if let ExprNode::Var { name, .. } = &*recv.node {
                out.entry(camel(name.as_str())).or_insert(format!(
                    "Dictionary<{}, {}>",
                    nn(index.ty.as_ref()),
                    nn(value.ty.as_ref())
                ));
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if matches!(method.as_str(), "<<" | "add" | "push") && args.len() == 1 =>
        {
            if let ExprNode::Var { name, .. } = &*r.node {
                out.entry(camel(name.as_str()))
                    .or_insert(format!("List<{}>", nn(args[0].ty.as_ref())));
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Var { name, .. } = &*r.node {
                out.entry(camel(name.as_str())).or_insert(format!(
                    "Dictionary<{}, {}>",
                    nn(args[0].ty.as_ref()),
                    nn(args[1].ty.as_ref())
                ));
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
    if let ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } = &*e.node {
        *counts.entry(camel(name.as_str())).or_insert(0) += 2;
    }
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
        let cn = camel(name.as_str());
        *counts.entry(cn.clone()).or_insert(0) += 1;
        if !nil_types.contains_key(&cn) {
            if let Some(ty) = value.ty.as_ref() {
                if !matches!(ty, crate::ty::Ty::Nil) {
                    let mut cs = csharp_ty(ty);
                    if !cs.ends_with('?') {
                        cs.push('?');
                    }
                    nil_types.insert(cn, cs);
                }
            }
        }
    }
    for child in children(e) {
        count_assigns(child, counts, nil_types);
    }
}

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

// ---- Expression rendering (value position) ----

pub fn emit_expr(e: &Expr) -> String {
    if let Some(s) = try_string_builder(e) {
        return s;
    }
    emit_node(&e.node, e)
}

pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

/// The view lowerer builds HTML by accumulating into a string buffer; C#
/// uses a `StringBuilder`:
///   - `Init`   `io = String.new` → `var io = new StringBuilder()`
///   - `Append` `io << chunk`     → `io.Append(chunk)`
///   - `Result` terminal `io`     → `io.ToString()`
fn try_string_builder(e: &Expr) -> Option<String> {
    match e.hint? {
        IrHint::StringBuilderInit => {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*e.node {
                return Some(format!("var {} = new StringBuilder()", camel(name.as_str())));
            }
            None
        }
        IrHint::StringBuilderAppend => {
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
                if method.as_str() == "<<" && args.len() == 1 {
                    if let ExprNode::Var { name, .. } = &*r.node {
                        return Some(format!(
                            "{}.Append({})",
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
                return Some(format!("{}.ToString()", camel(name.as_str())));
            }
            None
        }
    }
}

/// Emit a top-level constant's value.
pub fn emit_constant_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

/// Public entry for `wrap_return` (library.rs): render an expression for a
/// `return`. (C# infers map/list element types from the new-expression, so
/// no special re-typing is needed as in Kotlin.)
pub(super) fn emit_return_value(e: &Expr) -> String {
    emit_expr(e)
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
        ExprNode::Ivar { name } => {
            let n = camel(name.as_str());
            if is_object_tl_field(&n) {
                // `ThreadLocal<T>.Value` is annotated maybe-null; the field is
                // always initialized, so a null-forgiving read is safe.
                format!("{n}.Value!")
            } else {
                nonnull_read(ivar_name(name.as_str()))
            }
        }
        ExprNode::SelfRef => {
            if in_static() {
                CURRENT_CLASS.with(|c| c.borrow().clone())
            } else {
                "this".to_string()
            }
        }
        ExprNode::Const { path } => {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            qualify_colliding_const(type_name(&joined))
        }
        ExprNode::Hash { entries, .. } => emit_hash(entries, e),
        ExprNode::Array { elements, .. } => emit_array(elements, e),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, left, right, .. } => emit_bool_op(*op, left, right, e),
        ExprNode::Send { recv, method, args, block, .. } => {
            let rendered = emit_send(recv.as_ref(), method.as_str(), args, block.as_ref());
            coerce_nullable_finder(rendered, recv.as_ref(), method.as_str(), e.ty.as_ref())
        }
        // Value position: if → ternary, case → switch-expression.
        ExprNode::If { cond, then_branch, else_branch } => {
            emit_if_expr(cond, then_branch, else_branch)
        }
        ExprNode::Case { scrutinee, arms } => emit_case_expr(scrutinee, arms),
        // A Seq in value position: its value is the last element.
        ExprNode::Seq { exprs } => {
            exprs.last().map(emit_expr).unwrap_or_else(|| "null".to_string())
        }
        ExprNode::Cast { value, target_ty } => emit_cast(value, target_ty),
        ExprNode::Lambda { params, body, .. } => emit_lambda(params, body, false),
        ExprNode::Yield { args } => {
            format!("block({})", args.iter().map(emit_expr).collect::<Vec<_>>().join(", "))
        }
        ExprNode::RescueModifier { expr, fallback } => format!(
            "RhRuntime.Rescue(() => {}, () => {})",
            emit_expr(expr),
            emit_expr(fallback)
        ),
        // These don't appear in value position for the model subset; render
        // defensively.
        ExprNode::Return { .. }
        | ExprNode::While { .. }
        | ExprNode::Assign { .. }
        | ExprNode::OpAssign { .. }
        | ExprNode::Raise { .. }
        | ExprNode::Next { .. }
        | ExprNode::Break { .. }
        | ExprNode::Super { .. } => emit_stmt(e),
        other => format!("/* TODO {} */ null", other.kind_str()),
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        // `Ty::Int → long`; suffix `L` keeps integer literals in the
        // long-everywhere world (matches the `Db` primitive's `long` indices).
        Literal::Int { value } => format!("{value}L"),
        Literal::Float { value } => {
            if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                format!("{value}")
            }
        }
        Literal::Str { value } => format!("\"{}\"", escape_str(value)),
        Literal::Sym { value } => format!("\"{}\"", escape_str(value.as_str())),
        Literal::Regex { pattern, .. } => format!("new Regex(\"{}\")", escape_str(pattern)),
    }
}

/// Escape for a C# regular (non-interpolated) string literal.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

/// Escape literal text inside an interpolated string (`$"..."`): same as a
/// regular string, plus `{`/`}` are doubled.
fn escape_interp_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

fn emit_hash(entries: &[(Expr, Expr)], e: &Expr) -> String {
    if entries.is_empty() {
        if let Some(crate::ty::Ty::Hash { key, value }) = e.ty.as_ref() {
            return format!("new Dictionary<{}, {}>()", csharp_ty(key), csharp_ty(value));
        }
        return "new Dictionary<string, object?>()".to_string();
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("[{}] = {}", emit_expr(k), emit_expr(v)))
        .collect();
    // Heterogeneous `<string, object?>` so a mixed map type-checks against
    // `object?` params.
    format!("new Dictionary<string, object?> {{ {} }}", pairs.join(", "))
}

fn emit_array(elements: &[Expr], e: &Expr) -> String {
    let elem = match e.ty.as_ref() {
        Some(crate::ty::Ty::Array { elem }) => csharp_ty(elem),
        _ => "object?".to_string(),
    };
    if elements.is_empty() {
        return format!("new List<{elem}>()");
    }
    let els: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("new List<{elem}> {{ {} }}", els.join(", "))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("$\"");
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(&escape_interp_text(value)),
            InterpPart::Expr { expr } => {
                out.push_str(&format!("{{{}}}", emit_expr(expr)));
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
        // `||` is logical-or for Bool results; Ruby's `x || default`
        // nil-coalescing idiom maps to C#'s `??` when the result isn't Bool.
        BoolOpKind::Or => {
            if matches!(e.ty.as_ref(), Some(crate::ty::Ty::Bool)) {
                format!("{l} || {r}")
            } else {
                format!("({l} ?? {r})")
            }
        }
    }
}

/// `if` in value position → C# ternary. When the condition is an `is_a?`
/// narrowing (`x is T ? x : default`), the then-branch is cast to `T` — C#
/// doesn't auto-narrow `x` in the true arm the way Kotlin smart-casts.
fn emit_if_expr(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    let c = emit_expr(cond);
    let els = if is_empty_branch(else_branch) {
        "null".to_string()
    } else {
        emit_expr(else_branch)
    };
    if let ExprNode::Send { recv: Some(_), method, args, .. } = &*cond.node {
        if method.as_str() == "is_a?" && args.len() == 1 {
            if let ExprNode::Const { path } = &*args[0].node {
                if let Some(t) = is_a_cast_type(path.last().map(|s| s.as_str()).unwrap_or("")) {
                    return format!("({c} ? ({t})({}) : ({els}))", emit_expr(then_branch));
                }
            }
        }
    }
    format!("({c} ? ({}) : ({els}))", emit_expr(then_branch))
}

/// `case` in value position → C# switch-expression. Arms are cast to
/// `object?` so heterogeneous bodies (a model indexer's `long id` vs
/// `string title`) share a common result type.
fn emit_case_expr(scrutinee: &Expr, arms: &[Arm]) -> String {
    let s = emit_expr(scrutinee);
    let mut lines = Vec::new();
    let mut has_default = false;
    for arm in arms {
        let body = emit_expr(&arm.body);
        match &arm.pattern {
            Pattern::Wildcard | Pattern::Bind { .. } => {
                has_default = true;
                lines.push(format!("    _ => (object?)({body}),"));
            }
            Pattern::Lit { value } => {
                lines.push(format!("    {} => (object?)({body}),", emit_literal(value)));
            }
            other => {
                lines.push(format!("    /* TODO pattern {other:?} */ _ => (object?)({body}),"));
                has_default = true;
            }
        }
    }
    if !has_default {
        lines.push("    _ => null,".to_string());
    }
    format!("({s} switch {{\n{}\n}})", lines.join("\n"))
}

/// Run `f` with `props` added to `NONNULL_PROPS` (restoring afterward).
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

fn emit_cast(value: &Expr, target_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let v = emit_expr(value);
    match target_ty {
        Ty::Int => format!("Convert.ToInt64({v})"),
        Ty::Float => format!("Convert.ToDouble({v})"),
        Ty::Str | Ty::Sym => format!("(Convert.ToString({v}) ?? \"\")"),
        _ => format!("(({}){v})", csharp_ty(target_ty)),
    }
}

/// `recv[begin..]` / `recv[begin..end]` → C# `Substring`. Indices are
/// `long` (Ty::Int → long), so `(int)` for the String API.
fn emit_slice_range(
    rs: &str,
    begin: Option<&Expr>,
    end: Option<&Expr>,
    exclusive: bool,
) -> String {
    let b = begin.map(emit_expr).unwrap_or_else(|| "0L".to_string());
    match end {
        None => format!("{rs}.Substring((int)({b}))"),
        Some(e) => {
            let e = emit_expr(e);
            let len = if exclusive {
                format!("(int)(({e}) - ({b}))")
            } else {
                format!("(int)(({e}) - ({b}) + 1)")
            };
            format!("{rs}.Substring((int)({b}), {len})")
        }
    }
}

fn emit_lambda(params: &[crate::ident::Symbol], body: &Expr, _destructure: bool) -> String {
    let body_s = emit_expr(body);
    let ps: Vec<String> = params.iter().map(|p| camel(p.as_str())).collect();
    format!("({}) => {body_s}", ps.join(", "))
}

fn forces_parens(method: &str) -> bool {
    matches!(
        method,
        "save" | "save!" | "destroy" | "destroy!" | "reload" | "validate" | "dup" | "clone"
    )
}

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
    let is_model_stamp = match result_ty {
        Some(Ty::Class { .. }) => true,
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
            matches!(non_nil.as_slice(), [Ty::Class { .. }])
        }
        None | Some(Ty::Untyped) | Some(Ty::Var { .. }) => {
            let ExprNode::Const { path } = &*r.node else { return rendered };
            let cls = type_name(&path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::"));
            CLASS_HIERARCHY.with(|h| h.borrow().contains_key(&cls))
        }
        Some(_) => false,
    };
    if is_model_stamp {
        return format!("{rendered}!");
    }
    rendered
}

fn emit_raise(value: &Expr) -> String {
    match &*value.node {
        ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => {
            format!("throw new Exception({})", emit_expr(value))
        }
        _ => format!("throw {}", emit_expr(value)),
    }
}

fn arg_already_ty(arg: &Expr, target_ty: &crate::ty::Ty) -> bool {
    if let ExprNode::Cast { target_ty: t, .. } = &*arg.node {
        return t == target_ty;
    }
    arg.ty.as_ref() == Some(target_ty)
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
) -> String {
    if recv.is_none() && args.is_empty() && block.is_none() && is_param(method) {
        return camel(method);
    }

    if recv.is_none() && method == "!" && args.len() == 1 {
        return format!("!({})", emit_expr(&args[0]));
    }

    let args_s: Vec<String> = args.iter().map(emit_expr).collect();

    // `self.class.METHOD(...)` → unqualified `METHOD(...)` (the per-model
    // static method).
    if let Some(r) = recv {
        if let ExprNode::Send { recv: Some(inner), method: m2, args: a2, .. } = &*r.node {
            if m2.as_str() == "class"
                && a2.is_empty()
                && matches!(&*inner.node, ExprNode::SelfRef)
            {
                return format!("{}({})", pascal(method), args_s.join(", "));
            }
        }
    }

    // Stdlib module calls.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            match (path.last().map(|s| s.as_str()), method) {
                (Some("Base64"), "strict_encode64") => {
                    return format!(
                        "Convert.ToBase64String(System.Text.Encoding.UTF8.GetBytes({}))",
                        args_s[0]
                    );
                }
                (Some("JSON"), "generate") => {
                    return format!("JsonBuilder.EncodeValue({})", args_s[0]);
                }
                // Temporal reader intrinsic: `ActiveSupport.parse_db_time(s)`
                // parses stored ISO-8601 text into a native `DateTimeOffset`.
                // Nil-safe (`string?` → `DateTimeOffset?`), so the raw storage
                // backing passes straight through (no null-forgiving).
                (Some("ActiveSupport"), "parse_db_time") if args.len() == 1 => {
                    return format!("Roundhouse.RhDateTime.Parse({})", args_s[0]);
                }
                _ => {}
            }
        }
    }

    // Constructor: `X.new(...)` → `new X(...)`. Implicit-self `new(...)` → the
    // current class's constructor.
    if method == "new" {
        let is_method = method_params_for(recv, "new").is_some();
        if let Some(r) = recv {
            if is_method {
                return format!("{}.{}({})", emit_expr(r), pascal("new"), emit_call_args(recv, "new", args));
            }
            return format!("new {}({})", emit_expr(r), args_s.join(", "));
        }
        let cls = CURRENT_CLASS.with(|c| c.borrow().clone());
        if !cls.is_empty() {
            return format!("new {cls}({})", args_s.join(", "));
        }
    }

    // `raise Class, msg` → `throw new Class(msg)`.
    if method == "raise" && recv.is_none() && !args.is_empty() {
        if let ExprNode::Const { path } = &*args[0].node {
            let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            let cls = if joined.is_empty() {
                "Exception".to_string()
            } else {
                map_exception_class(&joined)
            };
            return format!("throw new {cls}({})", args_s[1..].join(", "));
        }
        return format!("throw new Exception({})", args_s.join(", "));
    }

    // Attribute setter: `recv.foo = v` arrives as a Send named `foo=`.
    if let (Some(r), 1) = (recv, args.len()) {
        if method.ends_with('=') && !matches!(method, "==" | "!=" | "<=" | ">=") {
            let base = &method[..method.len() - 1];
            if matches!(&*r.node, ExprNode::SelfRef) {
                if let Some(ty) = instance_prop_scalar_ty(base) {
                    if !arg_already_ty(&args[0], &ty) {
                        return format!("this.{} = {}", pascal(base), emit_cast(&args[0], &ty));
                    }
                }
            }
            return format!("{}.{} = {}", emit_expr(r), pascal(base), args_s[0]);
        }
    }

    // `is_a?(Class)` → C# `is` / boolean compare.
    if method == "is_a?" && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            let rs = emit_expr(r);
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            return match last {
                "TrueClass" => format!("({rs} as bool? == true)"),
                "FalseClass" => format!("({rs} as bool? == false)"),
                "Integer" => format!("({rs} is long)"),
                "Float" => format!("({rs} is double)"),
                "String" => format!("({rs} is string)"),
                "Numeric" => format!("({rs} is long || {rs} is double)"),
                "Hash" => format!("({rs} is System.Collections.IDictionary)"),
                "Array" => format!("({rs} is System.Collections.IList)"),
                other => format!("({rs} is {})", type_name(other)),
            };
        }
    }

    // `recv.gsub(regex, hash)` → `regex.Replace(recv, m => hash[m] ?? m)`.
    // The pattern is a `Regex` (constant/literal), so dispatch off it.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "{}.Replace({}, m => {}.GetValueOrDefault(m.Value, m.Value))",
                args_s[0],
                emit_expr(r),
                args_s[1]
            );
        }
    }

    // String predicates with one arg.
    if let (Some(r), 1) = (recv, args.len()) {
        match method {
            "start_with?" => return format!("{}.StartsWith({})", emit_expr(r), args_s[0]),
            "end_with?" => return format!("{}.EndsWith({})", emit_expr(r), args_s[0]),
            "include?" => return format!("{}.Contains({})", emit_expr(r), args_s[0]),
            "join" => return format!("string.Join({}, {})", args_s[0], emit_expr(r)),
            // `str.split(sep)` → C# `Split` materialized to a `List<string>`
            // (Ruby `split` yields an Array; the runtime treats it as one).
            "split" => return format!("{}.Split({}).ToList()", emit_expr(r), args_s[0]),
            _ => {}
        }
    }

    // Indexing / slicing.
    if method == "[]" {
        if let Some(r) = recv {
            let rs = emit_expr(r);
            if args.len() == 1 {
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    return emit_slice_range(&rs, begin.as_ref(), end.as_ref(), *exclusive);
                }
                if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Array { .. })) {
                    return format!("{rs}[(int)({})]", args_s[0]);
                }
                // Ruby `Hash#[]` returns nil for a missing key; C#'s Dictionary
                // indexer throws — read via `GetValueOrDefault` to match.
                if recv_is_hash(r) {
                    return format!("{rs}.GetValueOrDefault({})", args_s[0]);
                }
                return format!("{rs}[{}]", args_s[0]);
            }
            if args.len() == 2 {
                let start = &args_s[0];
                let len = &args_s[1];
                return format!("{rs}.Substring((int)({start}), (int)({len}))");
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
        if method == "<<" || method == "push" {
            return format!("{}.Add({})", emit_expr(r), args_s[0]);
        }
        if (method == "key?" || method == "has_key?") && recv_is_hash(r) {
            return format!("{}.ContainsKey({})", emit_expr(r), args_s[0]);
        }
        if method == "delete" && recv_is_hash(r) {
            return format!("{}.Remove({})", emit_expr(r), args_s[0]);
        }
        if method == "merge" {
            return format!(
                "RhRuntime.Merge({}, {})",
                emit_expr(r),
                args_s[0]
            );
        }
    }
    if let (Some(r), 2) = (recv, args.len()) {
        if method == "[]=" {
            return format!("{}[{}] = {}", emit_expr(r), args_s[0], args_s[1]);
        }
        if method == "fetch" {
            return format!("({}.GetValueOrDefault({}, {}))", emit_expr(r), args_s[0], args_s[1]);
        }
        if method == "tr" {
            return format!("{}.Replace({}, {})", emit_expr(r), args_s[0], args_s[1]);
        }
    }

    // Zero-arg receiver sends: builtin coercions, then property vs method.
    if let (Some(r), true) = (recv, args.is_empty() && block.is_none()) {
        let rs = emit_expr(r);
        match method {
            // A non-nullable value-typed receiver (`long`/`double`/`bool`
            // column) is never nil — emit `false` so C# doesn't warn on an
            // always-false `== null` comparison.
            "nil?" if matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Int | crate::ty::Ty::Float | crate::ty::Ty::Bool)
            ) =>
            {
                return "false".to_string()
            }
            "nil?" => return format!("({rs} == null)"),
            "!" => return format!("!({rs})"),
            "to_s" => return format!("(Convert.ToString({rs}) ?? \"\")"),
            "to_i" => return format!("Convert.ToInt64({rs})"),
            "to_f" => return format!("Convert.ToDouble({rs})"),
            "empty?" => {
                return if recv_is_array(r) || recv_is_hash(r) {
                    format!("({rs}.Count == 0)")
                } else {
                    format!("({rs}.Length == 0)")
                };
            }
            "any?" => {
                return if recv_is_array(r) || recv_is_hash(r) {
                    format!("({rs}.Count != 0)")
                } else {
                    format!("({rs}.Length != 0)")
                };
            }
            "upcase" => return format!("{rs}.ToUpperInvariant()"),
            "downcase" => return format!("{rs}.ToLowerInvariant()"),
            "strip" => return format!("{rs}.Trim()"),
            "join" => return format!("string.Join(\"\", {rs})"),
            "length" | "size" => {
                return if recv_is_array(r) || recv_is_hash(r) {
                    format!("(long){rs}.Count")
                } else {
                    format!("(long){rs}.Length")
                };
            }
            "count" if recv_is_array(r) || recv_is_hash(r) => {
                return format!("(long){rs}.Count");
            }
            "keys" if recv_is_hash(r) => return format!("{rs}.Keys.ToList()"),
            "values" if recv_is_hash(r) => return format!("{rs}.Values.ToList()"),
            "freeze" | "dup" | "to_a" => return rs,
            "to_h" if recv_is_hash(r) => return rs,
            _ => {}
        }
        if matches!(&*r.node, ExprNode::Const { .. }) {
            // `rs` may carry a collision-avoiding `Roundhouse.` qualifier; the
            // object-accessor registry is keyed by the bare type name.
            let obj = rs.strip_prefix("Roundhouse.").unwrap_or(rs.as_str());
            if is_object_prop(obj, method) {
                return format!("{rs}.{}", pascal(method));
            }
            return format!("{rs}.{}()", pascal(method));
        }
        if matches!(&*r.node, ExprNode::SelfRef) {
            return if is_instance_prop(method) {
                format!("{rs}.{}", pascal(method))
            } else {
                format!("{rs}.{}()", pascal(method))
            };
        }
        if let Some(cls) = receiver_class_name(r) {
            if is_instance_method_of(&cls, method) {
                return format!("{rs}.{}()", pascal(method));
            }
        }
        if !forces_parens(method) && !method.ends_with('?') && !method.ends_with('!') {
            return format!("{rs}.{}", pascal(method));
        }
    }

    // Block → C# lambda. `.each` maps to `.ForEach` on a List; on a Map it
    // iterates entries. On a user type it stays a method call.
    if let Some(b) = block {
        let recv_arr =
            recv.is_some_and(|r| ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Array { .. })));
        let recv_hash =
            recv.is_some_and(|r| ty_is(r.ty.as_ref(), |t| matches!(t, crate::ty::Ty::Hash { .. })));
        // `Hash#each { |k, v| … }` → a C# `foreach` over KeyValuePairs (the
        // body is statements, which a method-call lambda can't hold).
        // A 2-param block IS a hash iteration. A param/ivar of a statically
        // dictionary type (Flash/Session's `other`) yields typed
        // `KeyValuePair`s (`k`/`v` keep their element types — Flash's
        // `k == "notice"` needs `k: string`); anything else — a local that
        // may be C#-`object?` even when the IR narrows it to Hash
        // (render_attrs' nested `v`) — iterates the non-generic `IDictionary`
        // (object key/value). Each loop gets a unique entry var (nested
        // hash-each, e.g. render_attrs, would otherwise reuse `__kv`).
        if method == "each" {
            if let (Some(r), ExprNode::Lambda { params, body, .. }) = (recv, &*b.node) {
                if params.len() == 2 {
                    let k = camel(params[0].as_str());
                    let v = camel(params[1].as_str());
                    let typed = recv_hash
                        && match &*r.node {
                            ExprNode::Var { name, .. } => is_param(name.as_str()),
                            ExprNode::Ivar { .. } => true,
                            _ => false,
                        };
                    let entry = format!("__kv{}", next_loop_id());
                    let inner = indent(&emit_stmt(body));
                    if typed {
                        return format!(
                            "foreach (var {entry} in {}) {{\n    var {k} = {entry}.Key;\n    var {v} = {entry}.Value;\n{inner}\n}}",
                            emit_expr(r)
                        );
                    }
                    return format!(
                        "foreach (System.Collections.DictionaryEntry {entry} in (System.Collections.IDictionary)({})) {{\n    var {k} = {entry}.Key;\n    var {v} = {entry}.Value;\n{inner}\n}}",
                        emit_expr(r)
                    );
                }
            }
        }
        // `Array#each { |x| … }` → a C# `foreach` (the block body is
        // statements — preload grouping, destroy loops — which a `.ForEach`
        // expression-lambda can't hold).
        if method == "each" && recv_arr {
            if let (Some(r), ExprNode::Lambda { params, body, .. }) = (recv, &*b.node) {
                if params.len() == 1 {
                    let p = camel(params[0].as_str());
                    return format!(
                        "foreach (var {p} in {}) {{\n{}\n}}",
                        emit_expr(r),
                        indent(&emit_stmt(body))
                    );
                }
            }
        }
        let cs_method = if method == "each" && recv_arr {
            "ForEach".to_string()
        } else if method == "map" {
            "Select".to_string()
        } else {
            pascal(method)
        };
        let lam = match &*b.node {
            ExprNode::Lambda { params, body, .. } => emit_lambda(params, body, recv_hash),
            _ => emit_expr(b),
        };
        let base = match recv {
            Some(r) => format!("{}.{cs_method}", emit_expr(r)),
            None => cs_method,
        };
        // C# `.Select` is lazy and yields `IEnumerable`; materialize to match
        // the `List` model.
        let tail = if method == "map" { ".ToList()" } else { "" };
        if args_s.is_empty() {
            return format!("{base}({lam}){tail}");
        }
        return format!("{base}({}, {lam}){tail}", args_s.join(", "));
    }

    // General call. A trailing `kwargs: true` hash splats into C# named
    // arguments when the callee is known to have matching params.
    let name = pascal(method);
    let call_args = emit_call_args(recv, method, args);
    match recv {
        Some(r) => format!("{}.{name}({call_args})", emit_expr(r)),
        None => format!("{name}({call_args})"),
    }
}

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
                            parts.push(format!("{k}: {}", emit_expr(v)));
                        }
                        return parts.join(", ");
                    }
                }
            }
        }
    }
    args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
}

// ---- Statement rendering (body position; appends `;` / block forms) ----

/// Render an expression as a statement: control flow as a block, everything
/// else as an expression-statement terminated with `;`. Empty/`nil` no-ops
/// render to the empty string.
pub(super) fn emit_stmt(e: &Expr) -> String {
    // The view string-builder sites (`io = String.new`, `io << chunk`) are
    // statements — route them through `try_string_builder` so the init emits
    // `var io = new StringBuilder()` (else `io` is a bare `string`).
    if let Some(s) = try_string_builder(e) {
        return format!("{s};");
    }
    match &*e.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(emit_stmt)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::If { cond, then_branch, else_branch } => emit_if_stmt(cond, then_branch, else_branch),
        ExprNode::While { cond, body, until_form } => {
            let c = emit_expr(cond);
            let c = if *until_form { format!("!({c})") } else { c };
            format!("while ({c}) {{\n{}\n}}", indent(&emit_stmt(body)))
        }
        ExprNode::Case { scrutinee, arms } => emit_case_stmt(scrutinee, arms),
        ExprNode::Assign { target, value } => format!("{};", emit_assign(target, value)),
        ExprNode::OpAssign { target, op, value } => format!("{};", emit_op_assign(target, *op, value)),
        ExprNode::Return { value } => emit_return_stmt(value),
        ExprNode::Raise { value } => format!("{};", emit_raise(value)),
        ExprNode::Super { .. } => "/* super() */".to_string(),
        // Loop control (inside an emitted `while`/`foreach`).
        ExprNode::Next { .. } => "continue;".to_string(),
        ExprNode::Break { .. } => "break;".to_string(),
        // A bare value expression in statement position is a Ruby implicit-
        // return no-op (the method's trailing `value` / `self`); C# rejects it
        // as a statement (CS0201), so drop it. (A value in *return* position is
        // handled by `wrap_return`, not here.)
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => String::new(),
        // A bare expression used for effect.
        _ => {
            let s = emit_expr(e);
            if s.is_empty() {
                s
            } else {
                format!("{s};")
            }
        }
    }
}

fn emit_return_stmt(value: &Expr) -> String {
    let nil_in_unit = RETURNS_UNIT.with(|r| *r.borrow())
        && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
    if nil_in_unit {
        return "return;".to_string();
    }
    format!("return {};", emit_expr(value))
}

/// `if` in statement position → block form. Props the condition proves
/// non-null are read with `!` in the branch where they hold.
fn emit_if_stmt(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    // `is_a?` narrowing guard: `if (x is T) { …x… }` → `if (x is T xAsT)
    // { …xAsT… }`. C# doesn't narrow `x` in the true arm without a pattern
    // variable (and can't reuse `x`'s name), so bind a fresh name and rewrite
    // the then-branch's references. Only the then-branch narrows.
    if let Some((var, ty)) = narrowing_guard(cond) {
        let patvar = format!("{var}As{}", sanitize_type(&ty));
        let then = rename_word(&emit_stmt(then_branch), &var, &patvar);
        let head = format!("if ({var} is {ty} {patvar}) {{\n{}\n}}", indent(&then));
        if is_empty_branch(else_branch) {
            return head;
        }
        return format!("{head} else {{\n{}\n}}", indent(&emit_stmt(else_branch)));
    }
    let c = emit_expr(cond);
    let (mut then_nn, mut else_nn) = (Vec::new(), Vec::new());
    guarded_nonnull(cond, &mut then_nn, &mut else_nn);
    let then = with_nonnull(&then_nn, || indent(&emit_stmt(then_branch)));
    if is_empty_branch(else_branch) {
        format!("if ({c}) {{\n{then}\n}}")
    } else {
        let els = with_nonnull(&else_nn, || indent(&emit_stmt(else_branch)));
        format!("if ({c}) {{\n{then}\n}} else {{\n{els}\n}}")
    }
}

/// An `is_a?(Var, Class)` guard with a clean cast type → `(camelCased var,
/// C# type)`. Only bare-`Var` receivers narrow (a param/local).
fn narrowing_guard(cond: &Expr) -> Option<(String, String)> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*cond.node {
        if method.as_str() == "is_a?" && args.len() == 1 {
            if let (ExprNode::Var { name, .. }, ExprNode::Const { path }) =
                (&*r.node, &*args[0].node)
            {
                if let Some(t) = is_a_cast_type(path.last().map(|s| s.as_str()).unwrap_or("")) {
                    return Some((camel(name.as_str()), t));
                }
            }
        }
    }
    None
}

/// A C# type → an identifier-safe suffix for the pattern variable
/// (`string`→`String`, `long`→`Long`, `Article`→`Article`).
fn sanitize_type(t: &str) -> String {
    let cleaned: String = t.chars().filter(|c| c.is_alphanumeric()).collect();
    let mut chars = cleaned.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => "X".to_string(),
    }
}

/// Whole-word identifier replace (narrowed-var rewrite). Safe here: the
/// emitted branch references the var only as a bare token.
fn rename_word(body: &str, from: &str, to: &str) -> String {
    let bytes = body.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if body[i..].starts_with(from)
            && (i == 0 || !is_word(bytes[i - 1]))
            && (i + from.len() >= body.len() || !is_word(bytes[i + from.len()]))
        {
            out.push_str(to);
            i += from.len();
        } else {
            let ch = body[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// `case` in statement position → an `if`/`else if` chain on scrutinee
/// equality (arms have statement bodies, which a switch-expression can't
/// hold).
fn emit_case_stmt(scrutinee: &Expr, arms: &[Arm]) -> String {
    let s = emit_expr(scrutinee);
    let mut out = String::new();
    let mut first = true;
    let mut else_body: Option<String> = None;
    for arm in arms {
        let body = indent(&emit_stmt(&arm.body));
        match &arm.pattern {
            Pattern::Wildcard | Pattern::Bind { .. } => {
                if !body.trim().is_empty() {
                    else_body = Some(body);
                }
            }
            Pattern::Lit { value } => {
                let kw = if first { "if" } else { "else if" };
                first = false;
                out.push_str(&format!("{kw} ({s} == {}) {{\n{body}\n}} ", emit_literal(value)));
            }
            other => {
                let kw = if first { "if" } else { "else if" };
                first = false;
                out.push_str(&format!(
                    "{kw} (false) {{ /* TODO pattern {other:?} */\n{body}\n}} "
                ));
            }
        }
    }
    if let Some(eb) = else_body {
        if first {
            // Only a default arm.
            return eb;
        }
        out.push_str(&format!("else {{\n{eb}\n}}"));
    }
    out.trim_end().to_string()
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
                DECLARED.with(|d| {
                    d.borrow_mut().insert(n.clone());
                });
                let empty_hash =
                    matches!(&*value.node, ExprNode::Hash { entries, .. } if entries.is_empty());
                let empty_arr =
                    matches!(&*value.node, ExprNode::Array { elements, .. } if elements.is_empty());
                if empty_hash || empty_arr {
                    if let Some(t) = CONTAINER_TYPES.with(|c| c.borrow().get(&n).cloned()) {
                        return format!("var {n} = new {t}()");
                    }
                }
                let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
                if is_nil {
                    if let Some(ty) = NIL_TYPES.with(|t| t.borrow().get(&n).cloned()) {
                        return format!("{ty} {n} = {val}");
                    }
                    return format!("object? {n} = {val}");
                }
                format!("var {n} = {val}")
            }
        }
        LValue::Ivar { name } if is_object_tl_field(&camel(name.as_str())) => {
            // Reset to an empty container → target-typed `new()` so it adopts
            // the thread-local's declared element type (the `@slots` value type
            // is nullable; an explicit `new Dictionary<string,string>()` would
            // mismatch).
            let rhs = if matches!(&*value.node, ExprNode::Hash { entries, .. } if entries.is_empty())
                || matches!(&*value.node, ExprNode::Array { elements, .. } if elements.is_empty())
            {
                "new()".to_string()
            } else {
                val
            };
            format!("{}.Value = {rhs}", camel(name.as_str()))
        }
        _ => format!("{} = {val}", lvalue_ref(target)),
    }
}

fn lvalue_ref(target: &LValue) -> String {
    match target {
        LValue::Var { name, .. } => camel(name.as_str()),
        LValue::Ivar { name } => format!("this.{}", ivar_name(name.as_str())),
        LValue::Attr { recv, name } => format!("{}.{}", emit_expr(recv), pascal(name.as_str())),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
        LValue::Const { path } => path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("."),
    }
}

fn emit_op_assign(target: &LValue, op: OpAssignOp, value: &Expr) -> String {
    let lhs = lvalue_ref(target);
    let v = emit_expr(value);
    match op {
        OpAssignOp::OrOr => format!("{lhs} = {lhs} ?? {v}"),
        OpAssignOp::AndAnd => format!("if ({lhs} != null) {{ {lhs} = {v}; }}"),
        OpAssignOp::Add => format!("{lhs} += {v}"),
        OpAssignOp::Sub => format!("{lhs} -= {v}"),
        OpAssignOp::Mul => format!("{lhs} *= {v}"),
        OpAssignOp::Div => format!("{lhs} /= {v}"),
        OpAssignOp::Mod => format!("{lhs} %= {v}"),
        _ => format!("{lhs} = {lhs} /* TODO op-assign */ {v}"),
    }
}
