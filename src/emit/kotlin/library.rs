//! `LibraryClass` → Kotlin file.
//!
//! Renders a lowered class to idiomatic Kotlin:
//!   - `attr_reader`/`attr_writer` accessor methods collapse into Kotlin
//!     `var` properties (the property *is* the accessor); the synthetic
//!     getter/setter `MethodDef`s are dropped.
//!   - Instance `Method`s → `fun`; class methods (`def self.x`) →
//!     `companion object` members.
//!   - Ruby's implicit return becomes an explicit `return` on the final
//!     statement of value-returning methods (Kotlin block bodies don't
//!     implicitly return).
//!   - Kotlin requires every parameter typed, so params take their
//!     signature type, falling back to `Any?`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::expr::{
    begin_method, emit_expr, set_current_class, set_instance_prop_types, set_instance_props,
    set_param_names, set_return_label, set_returns_unit,
};
use super::naming::camel;
use super::ty::kotlin_ty;

/// Emit a `LibraryClass` as a standalone Kotlin file under
/// `src/main/kotlin/app/models/<Name>.kt`.
pub fn emit_class_file(lc: &LibraryClass) -> EmittedFile {
    let name = lc.name.0.as_str();
    let last = name.rsplit("::").next().unwrap_or(name);
    EmittedFile {
        path: PathBuf::from(format!("src/main/kotlin/app/models/{last}.kt")),
        content: format!("package roundhouse\n\n{}", emit_library_class(lc)),
    }
}

/// `Result`-returning wrapper for the `runtime_loader::TargetEmit` slot.
pub fn emit_library_class_result(lc: &LibraryClass) -> Result<String, String> {
    Ok(emit_library_class(lc))
}

/// Emit the app's route helpers (`new_article_path`, `article_path(id)`, …)
/// — lowered to `LibraryFunction`s sharing the `RouteHelpers` module path —
/// as a Kotlin `object RouteHelpers`. Returns `None` when the app has no
/// routes. The functions are class-method-shaped (no instance state), so
/// they reuse the `object` (module) emit path.
pub fn emit_route_helpers(funcs: &[crate::dialect::LibraryFunction]) -> Option<EmittedFile> {
    if funcs.is_empty() {
        return None;
    }
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| {
            let enclosing =
                f.module_path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            MethodDef {
                name: f.name.clone(),
                receiver: MethodReceiver::Class,
                params: f.params.clone(),
                block_param: None,
                body: f.body.clone(),
                signature: f.signature.clone(),
                effects: f.effects.clone(),
                enclosing_class: Some(crate::ident::Symbol::from(enclosing)),
                kind: AccessorKind::Method,
                is_async: f.is_async,
                mutates_self: false,
            }
        })
        .collect();
    let content = emit_module(&methods).ok()?;
    Some(EmittedFile {
        path: PathBuf::from("src/main/kotlin/RouteHelpers.kt"),
        content: format!("package roundhouse\n\n{content}"),
    })
}

/// Render a Ruby `module X` (parsed as a set of class methods) as a
/// Kotlin `object X { ... }`. Used for `Mode::Module` runtime entries
/// (e.g. `inflector.rb`). The module name comes from the methods'
/// `enclosing_class`.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    set_instance_prop_types(std::collections::HashMap::new());
    set_current_class("");
    let name = methods
        .first()
        .and_then(|m| m.enclosing_class.as_ref())
        .map(|s| s.as_str().rsplit("::").next().unwrap_or(s.as_str()).to_string())
        .unwrap_or_default();
    let mut out = format!("object {name} {{\n");
    // Module-level `@ivar` state (e.g. ViewHelpers' `@slots = {}`) → private
    // object properties; reads of them in method bodies resolve as property
    // names (set via `INSTANCE_PROPS`).
    let ivars = emit_object_body_ivars(methods, &BTreeMap::new());
    out.push_str(&ivars);
    if !ivars.is_empty() {
        out.push('\n');
    }
    for m in methods {
        out.push_str(&indent_method(&emit_method(m, "")));
        out.push('\n');
    }
    out.push_str("}\n");
    Ok(out)
}

/// Emit `private var <ivar>: T = <default>` declarations for module-level
/// `@ivar`s referenced in `methods`, skipping any already covered by
/// `accessor_props`. Also registers the ivar names as `INSTANCE_PROPS` so
/// body references read as properties (not method calls). Returns the
/// declaration block (each line indented), empty when there are none.
fn emit_object_body_ivars(
    methods: &[MethodDef],
    accessor_props: &BTreeMap<String, Ty>,
) -> String {
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in methods {
        collect_ivars(&m.body, &mut body_ivars);
    }
    set_instance_props(body_ivars.keys().cloned().collect());
    let inferred = infer_body_ivar_types(methods);
    let mut out = String::new();
    for n in body_ivars.keys() {
        if accessor_props.contains_key(n) {
            continue;
        }
        match inferred.get(n) {
            Some(ty) => {
                out.push_str(&format!("    private var {n}: {} = {}\n", kotlin_ty(ty), default_for(ty)))
            }
            None => out.push_str(&format!("    private var {n}: Any? = null\n")),
        }
    }
    out
}

pub fn emit_library_class(lc: &LibraryClass) -> String {
    let name = lc.name.0.as_str();
    let class_name = name.rsplit("::").next().unwrap_or(name).to_string();

    // A Ruby `module` (only module-functions, no instance state) → a
    // Kotlin `object`. Class-level `attr_accessor` (from `class << self`)
    // collapses to an object `var` property; everything else is a `fun`.
    if lc.is_module {
        set_instance_prop_types(std::collections::HashMap::new());
        set_current_class("");
        let accessor_props = class_accessor_props(&lc.methods);
        let mut out = format!("object {class_name} {{\n");
        for (n, ty) in &accessor_props {
            out.push_str(&format!("    {}\n", object_property_decl(n, ty)));
        }
        // Module-level `@ivar`s (e.g. ViewHelpers' `@slots = {}`) become
        // private object properties; this also registers them as
        // `INSTANCE_PROPS` so body references read as properties.
        let ivars = emit_object_body_ivars(&lc.methods, &accessor_props);
        out.push_str(&ivars);
        if !accessor_props.is_empty() || !ivars.is_empty() {
            out.push('\n');
        }
        for m in &lc.methods {
            // Skip the synthetic getter/setter funs — the `var` is the
            // accessor.
            if m.kind == AccessorKind::Method {
                out.push_str(&indent_method(&emit_method(m, "")));
                out.push('\n');
            }
        }
        out.push_str("}\n");
        return out;
    }

    // 1. Accessor-derived properties (name → type), and the set of method
    //    names to drop (the synthesized getters/setters).
    let mut prop_types: BTreeMap<String, Ty> = BTreeMap::new();
    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    prop_types
                        .entry(camel(m.name.as_str()))
                        .or_insert_with(|| (**ret).clone());
                }
            }
            AccessorKind::AttributeWriter => {
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    if let Some(p) = params.first() {
                        // writer name is `foo=`; strip the `=`.
                        let base = m.name.as_str().trim_end_matches('=');
                        prop_types.entry(camel(base)).or_insert_with(|| p.ty.clone());
                    }
                }
            }
            AccessorKind::Method => {}
        }
    }

    // 2. Body-only ivars (e.g. `@comments_cache`) that have no accessor —
    //    declared as `Any?` since we have no signature for them.
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }

    let mut out = String::new();

    // Ruby `initialize` → Kotlin primary constructor + `init` block. The
    // constructor params shadow the same-named properties inside `init`,
    // where ivar writes are `this.`-qualified.
    let init = lc
        .methods
        .iter()
        .find(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "initialize");

    // Parent class. Ruby's StandardError/RuntimeError → Kotlin
    // RuntimeException. A `super(args)` inside `initialize` becomes the
    // supertype constructor call in the header.
    let parent_name = lc.parent.as_ref().map(|p| {
        let last = p.0.as_str().rsplit("::").next().unwrap_or(p.0.as_str());
        match last {
            "StandardError" | "RuntimeError" => "RuntimeException".to_string(),
            other => other.to_string(),
        }
    });
    let super_args = init.and_then(|m| find_super_args(&m.body));
    let parent_clause = match (&parent_name, &super_args) {
        (Some(pn), Some(args)) => format!(" : {pn}({})", args.join(", ")),
        (Some(pn), None) => format!(" : {pn}()"),
        (None, _) => String::new(),
    };
    // Every emitted class is `open` — Kotlin classes are final by default,
    // and the model chain (Article → ApplicationRecord → Base) needs each
    // link extendable. The instance members a subclass inherits get an
    // explicit `override`; the rest are `open` so a further subclass could
    // override them (harmless on leaf classes). `inherited` is the union of
    // member names visible from the parent upward.
    let inherited: HashSet<String> = lc
        .parent
        .as_ref()
        .map(|p| p.0.as_str().rsplit("::").next().unwrap_or(p.0.as_str()))
        .map(super::expr::ancestor_members)
        .unwrap_or_default();
    let member_modifier = |name: &str| -> &'static str {
        if inherited.contains(name) {
            "override "
        } else {
            "open "
        }
    };

    let header = match init {
        Some(m) => {
            format!("open class {class_name}({}){parent_clause}", method_params(m).join(", "))
        }
        None => format!("open class {class_name}{parent_clause}"),
    };
    out.push_str(&header);
    out.push_str(" {\n");

    // Properties. Constructor-param-backed properties are assigned in the
    // `init` block, so they need no initializer (and a non-null type like
    // `Base` can't be defaulted to null anyway).
    let ctor_param_names: std::collections::HashSet<String> = init
        .map(|m| m.params.iter().map(|p| camel(p.name.as_str())).collect())
        .unwrap_or_default();
    for (n, ty) in &prop_types {
        if ctor_param_names.contains(n) {
            // Constructor-param-backed: assigned in the `init` block, no
            // declaration initializer. Kotlin forbids `open` on a
            // backing-field property without an initializer, so these stay
            // final (only the rare inherited case takes `override`).
            let m = if inherited.contains(n) { "override " } else { "" };
            out.push_str(&format!("    {m}var {n}: {}\n", kotlin_ty(ty)));
        } else {
            let m = member_modifier(n);
            out.push_str(&format!("    {m}var {n}: {} = {}\n", kotlin_ty(ty), default_for(ty)));
        }
    }
    let inferred_ivar_types = infer_body_ivar_types(&lc.methods);
    for n in body_ivars.keys() {
        if !prop_types.contains_key(n) {
            let m = member_modifier(n);
            match inferred_ivar_types.get(n) {
                Some(ty) => out.push_str(&format!(
                    "    {m}var {n}: {} = {}\n",
                    kotlin_ty(ty),
                    default_for(ty)
                )),
                None => out.push_str(&format!("    {m}var {n}: Any? = null\n")),
            }
        }
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() {
        out.push('\n');
    }

    // The class's property names (accessor-backed + body ivars), so a
    // `self.x` zero-arg send in a body emits as a property read; everything
    // else gets `()`. Active for the rest of this function's method emit.
    let instance_props: HashSet<String> =
        prop_types.keys().chain(body_ivars.keys()).cloned().collect();
    set_instance_props(instance_props);
    // Column scalar types, so a `self.<col> = row[k]`/`attrs[k]` write
    // coerces the untyped value to the property's type.
    set_instance_prop_types(prop_types.iter().map(|(n, t)| (n.clone(), t.clone())).collect());
    // Implicit-self `new(...)` in a companion factory → this class's ctor.
    set_current_class(&class_name);

    // init block (initialize body). Kotlin `init` can't `return`, so when
    // the body has a guard `return`, wrap it in `run { }` and emit
    // `return@run`.
    if let Some(m) = init {
        begin_method(&m.body);
        set_param_names(m.params.iter().map(|p| camel(p.name.as_str())).collect());
        set_returns_unit(true);
        let has_return = body_has_return(&m.body);
        if has_return {
            set_return_label(Some("run"));
        }
        let body = emit_body(&m.body, false);
        set_return_label(None);
        let inner = if has_return {
            format!("run {{\n{}\n}}", indent4(&body))
        } else {
            body
        };
        out.push_str(&format!("    init {{\n{}\n    }}\n\n", indent4(&indent4(&inner))));
    }

    // Instance methods (skip accessors and the initialize we just used).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance
            && m.kind == AccessorKind::Method
            && m.name.as_str() != "initialize"
        {
            out.push_str(&indent_method(&emit_method(m, member_modifier(&member_name(m)))));
            out.push('\n');
        }
    }

    // Class methods → companion object.
    let class_methods: Vec<&MethodDef> = lc
        .methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .collect();
    // Ruby's class-name reflection (`self.name`, emitted as a `name()`
    // call) has no Kotlin analog; synthesize a companion `name()` returning
    // this class's (Ruby-qualified) name. Skip if the class defines its own
    // `name`. Per-model subclasses get their own when emitted.
    let needs_name = references_class_name(&lc.methods)
        && !lc.methods.iter().any(|m| m.name.as_str() == "name");
    // Kotlin companions aren't inherited, so a model's `Article.find` /
    // `Article.exists` (the public AR finders Base defines) don't resolve
    // through the `: Base()` extends. Synthesize per-model copies that
    // delegate to the model's own `_adapter_*` companion methods (Db-direct
    // Level-3). Gated on the model marker `_adapterAll`; each emitted only
    // when not already defined (so Base, which defines them itself, and
    // ApplicationRecord, which has no `_adapter_*`, synthesize nothing).
    let present: HashSet<String> = class_methods.iter().map(|m| member_name(m)).collect();
    let synth_finders = synth_inherited_finders(&class_name, &present);
    if !class_methods.is_empty() || needs_name || !synth_finders.is_empty() {
        out.push_str("    companion object {\n");
        if needs_name {
            out.push_str(&format!(
                "        fun name(): String {{\n            return {:?}\n        }}\n",
                lc.name.0.as_str()
            ));
        }
        for m in class_methods {
            out.push_str(&indent_method(&indent_method(&emit_method(m, ""))));
            out.push('\n');
        }
        out.push_str(&synth_finders);
        out.push_str("    }\n");
    }

    out.push_str("}\n");
    out
}

/// Per-model copies of the public AR class methods Base defines, delegating
/// to the model's own `_adapter_*` companion members (Kotlin companions
/// aren't inherited). Emitted (indented for a companion body) only for a
/// Base-subclass model — detected by the `_adapterAll` marker — and only
/// for finders the class doesn't already define. `where`/`find_by` are
/// intentionally omitted (they route through the dropped adapter and
/// real-blog never calls them on a model). The bodies mirror
/// `active_record/base.rb`, specialized to `T` and using `size - 1` for
/// `last` (no negative index).
fn synth_inherited_finders(t: &str, present: &HashSet<String>) -> String {
    if !present.contains("_adapterAll") {
        return String::new();
    }
    let mut out = String::new();
    let mut emit = |name: &str, body: String| {
        if !present.contains(name) {
            out.push_str(&indent_method(&indent_method(&body)));
            out.push('\n');
        }
    };
    emit("all", format!("fun all(): MutableList<{t}> {{\n    return _adapterAll()\n}}\n"));
    emit(
        "find",
        format!(
            "fun find(id: Long): {t} {{\n    val result = _adapterFindById(id)\n    if (result == null) {{\n        throw RecordNotFound(\"Couldn't find {t} with id=${{id}}\")\n    }}\n    return result\n}}\n"
        ),
    );
    emit("count", "fun count(): Long {\n    return _adapterCount()\n}\n".to_string());
    emit(
        "exists",
        "fun exists(id: Long): Boolean {\n    return _adapterExistsById(id)\n}\n".to_string(),
    );
    emit(
        "last",
        format!(
            "fun last(): {t}? {{\n    val records = all()\n    return if (records.isEmpty()) null else records[records.size - 1]\n}}\n"
        ),
    );
    emit(
        "destroyAll",
        format!(
            "fun destroyAll(): MutableList<{t}> {{\n    val records = all()\n    records.forEach {{ it.destroy() }}\n    return records\n}}\n"
        ),
    );
    emit(
        "create",
        format!(
            "fun create(attrs: MutableMap<String, Any?> = mutableMapOf<String, Any?>()): {t} {{\n    val instance = {t}(attrs)\n    instance.save()\n    return instance\n}}\n"
        ),
    );
    emit(
        "createBang",
        format!(
            "fun createBang(attrs: MutableMap<String, Any?> = mutableMapOf<String, Any?>()): {t} {{\n    val instance = {t}(attrs)\n    if (!instance.save()) {{\n        throw RecordInvalid(instance)\n    }}\n    return instance\n}}\n"
        ),
    );
    out
}

fn indent_method(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Kotlin member name a method emits under (`[]`→`get`, `[]=`→`set`,
/// else camelCased) — the key used for override resolution.
fn member_name(m: &MethodDef) -> String {
    match m.name.as_str() {
        "[]" => "get".to_string(),
        "[]=" => "set".to_string(),
        _ => camel(m.name.as_str()),
    }
}

/// The camelCased instance-member names a class defines (accessor props +
/// body ivars + instance methods, excluding `initialize`). Used both to
/// register the class for override resolution and — via the ancestor union
/// — to decide which members of a subclass need `override`.
fn instance_member_names(lc: &LibraryClass) -> HashSet<String> {
    let mut out = HashSet::new();
    for m in &lc.methods {
        if m.receiver != MethodReceiver::Instance {
            continue;
        }
        match m.kind {
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter => {
                out.insert(camel(m.name.as_str().trim_end_matches('=')));
            }
            AccessorKind::Method if m.name.as_str() != "initialize" => {
                out.insert(member_name(m));
            }
            AccessorKind::Method => {}
        }
    }
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }
    out.extend(body_ivars.into_keys());
    out
}

/// Pre-scan hook: register each class's parent + instance members so that,
/// when a subclass renders, members it inherits get an `override` modifier
/// (Kotlin requires it explicitly). Skips modules (`object`s never
/// participate in inheritance). Called for the runtime classes (via the
/// `kotlin_units` transform) and the model classes (before they render).
pub fn register_class_hierarchy(classes: &[LibraryClass]) {
    for lc in classes {
        if lc.is_module {
            continue;
        }
        let name = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        let parent = lc
            .parent
            .as_ref()
            .map(|p| p.0.as_str().rsplit("::").next().unwrap_or(p.0.as_str()).to_string());
        super::expr::register_class_hierarchy(name, parent.as_deref(), instance_member_names(lc));
        super::expr::register_instance_methods(name, instance_method_names(lc));
    }
}

/// The camelCased names of a class's zero-arg instance *methods* (kind
/// `Method`, excluding `initialize` and any that take parameters) — the set
/// a typed-receiver send consults to keep its `()` (vs reading a property).
/// Distinct from `instance_member_names`, which also includes accessor
/// properties (used for `override` resolution).
fn instance_method_names(lc: &LibraryClass) -> HashSet<String> {
    lc.methods
        .iter()
        .filter(|m| {
            m.receiver == MethodReceiver::Instance
                && m.kind == AccessorKind::Method
                && m.name.as_str() != "initialize"
                && m.params.is_empty()
        })
        .map(member_name)
        .collect()
}

fn emit_method(m: &MethodDef, modifier: &str) -> String {
    // Ruby `[]` / `[]=` → Kotlin indexing operators. `set` is always
    // Unit-returning (the source RBS union return is dropped).
    let (decl_kw, name, force_unit) = match m.name.as_str() {
        "[]" => ("operator fun", "get".to_string(), false),
        "[]=" => ("operator fun", "set".to_string(), true),
        _ => ("fun", camel(m.name.as_str()), false),
    };

    let mut params = method_params(m);
    // A method that `yield`s takes a `block` parameter in Kotlin (there's
    // no implicit block); `yield` calls it. Type from the signature's
    // block slot.
    if body_has_yield(&m.body) {
        let bt = match m.signature.as_ref() {
            Some(Ty::Fn { block: Some(b), .. }) => kotlin_ty(b),
            _ => "(Any?) -> Unit".to_string(),
        };
        params.push(format!("block: {bt}"));
    }

    // Return type.
    let ret_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let returns_value = !force_unit && matches!(&ret_ty, Some(t) if !matches!(t, Ty::Nil));
    let ret_clause = if force_unit {
        String::new()
    } else {
        match &ret_ty {
            Some(t) if !matches!(t, Ty::Nil) => format!(": {}", kotlin_ty(t)),
            _ => String::new(),
        }
    };

    begin_method(&m.body);
    set_param_names(m.params.iter().map(|p| camel(p.name.as_str())).collect());
    // A `Unit` method's guard `return nil` emits a bare `return`.
    set_returns_unit(!returns_value);
    // A value-returning method with an empty body (Ruby `def x; end` →
    // implicit `nil`) can't emit a bare `return` in Kotlin — a non-Unit
    // function must yield a value. These are the load-bearing-empty AR
    // overrides (`_adapter_insert`, `_adapter_reload`, …); subclasses
    // override, so the base body never runs. Synthesize the type's default
    // (`0` for `Long`, `null` for nullable returns) to keep it a no-op.
    let body = if returns_value && is_empty_body(&m.body) {
        let ret = ret_ty.clone().unwrap_or(Ty::Untyped);
        format!("return {}", default_for(&ret))
    } else {
        emit_body(&m.body, returns_value)
    };

    format!(
        "{modifier}{decl_kw} {name}({}){ret_clause} {{\n{}\n}}\n",
        params.join(", "),
        indent4(&body)
    )
}

/// Render a method's params, always typed (Kotlin requirement); falls
/// back to `Any?` where the signature is missing.
fn method_params(m: &MethodDef) -> Vec<String> {
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    };
    m.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pn = camel(p.name.as_str());
            let ty = sig_params
                .and_then(|sp| sp.get(i))
                .map(|sp| kotlin_ty(&sp.ty))
                .unwrap_or_else(|| "Any?".to_string());
            match &p.default {
                Some(d) => format!("{pn}: {ty} = {}", emit_expr(d)),
                None => format!("{pn}: {ty}"),
            }
        })
        .collect()
}

fn indent4(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit a method body, adding an explicit `return` to the final statement
/// when the method returns a value (Ruby implicit return → Kotlin).
fn emit_body(body: &Expr, returns_value: bool) -> String {
    if !returns_value {
        return emit_expr(body);
    }
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = exprs[..exprs.len() - 1].iter().map(emit_expr).collect();
            lines.push(wrap_return(&exprs[exprs.len() - 1]));
            lines.join("\n")
        }
        _ => wrap_return(body),
    }
}

/// Prefix `return` unless the expression is already terminal or is a
/// statement that has no value (assignment, loop).
fn wrap_return(e: &Expr) -> String {
    // A nested `Seq` (e.g. the `else` block of a guard-return method) has
    // its *last* statement as its value — recurse so `return` lands there,
    // not on the whole block. Without this, a multi-statement tail emits
    // `return <stmt1>\n<stmt2>…` (the `return val stmt = …` bug).
    if let ExprNode::Seq { exprs } = &*e.node {
        if !exprs.is_empty() {
            return emit_body(e, true);
        }
    }
    // An empty `{}` / `[]` literal in return position: emit the
    // type-argument-free constructor so the method's declared return type
    // drives inference (`attributes()` → `MutableMap<String, Any?>`),
    // rather than the literal's own `<Any?, Any?>` guess.
    match &*e.node {
        ExprNode::Hash { entries, .. } if entries.is_empty() => {
            return "return mutableMapOf()".to_string();
        }
        ExprNode::Array { elements, .. } if elements.is_empty() => {
            return "return mutableListOf()".to_string();
        }
        _ => {}
    }
    let s = emit_expr(e);
    // A `raise Class, msg` send emits as a `throw` (type `Nothing`); like a
    // `Raise` node it needs no `return` prefix.
    let is_raise_send = matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "raise"
    );
    let no_return = is_raise_send
        || matches!(
            &*e.node,
            ExprNode::Return { .. }
                | ExprNode::Raise { .. }
                | ExprNode::While { .. }
                | ExprNode::Assign { .. }
                | ExprNode::Super { .. }
                | ExprNode::Next { .. }
                | ExprNode::Break { .. }
        );
    if no_return {
        s
    } else {
        format!("return {s}")
    }
}

fn body_has_yield(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Yield { .. }) || expr_children(e).iter().any(|c| body_has_yield(c))
}

/// Find a `super(args)` call (delegated to the parent constructor in the
/// class header). Returns the emitted arg strings, or `None` if there's
/// no `super` (or it's `super()` with no args returns `Some(vec![])`).
fn find_super_args(e: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Super { args } = &*e.node {
        return Some(
            args.as_ref()
                .map(|a| a.iter().map(emit_expr).collect())
                .unwrap_or_default(),
        );
    }
    for c in expr_children(e) {
        if let Some(r) = find_super_args(c) {
            return Some(r);
        }
    }
    None
}

fn body_has_return(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Return { .. })
        || expr_children(e).iter().any(|c| body_has_return(c))
}

fn collect_ivars(e: &Expr, out: &mut BTreeMap<String, ()>) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            out.insert(camel(name.as_str()), ());
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            out.insert(camel(name.as_str()), ());
            collect_ivars(value, out);
        }
        _ => {}
    }
    for child in expr_children(e) {
        collect_ivars(child, out);
    }
}

fn expr_children(e: &Expr) -> Vec<&Expr> {
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
        ExprNode::Hash { entries, .. } => {
            for (k, val) in entries {
                v.push(k);
                v.push(val);
            }
        }
        ExprNode::Array { elements, .. } => v.extend(elements.iter()),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    v.push(expr);
                }
            }
        }
        _ => {}
    }
    v
}

/// Pre-scan hook (runs before rendering, via the `kotlin_units` transform):
/// register every module/object-level accessor property so a
/// `Const`-receiver read of it (`ActiveRecord.adapter`) drops its parens.
/// Order-independent — the registry is populated for all classes in an
/// entry before any of them render.
pub fn register_object_accessors(classes: &[LibraryClass]) {
    for lc in classes {
        if !lc.is_module {
            continue;
        }
        let object = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        for prop in class_accessor_props(&lc.methods).keys() {
            super::expr::register_object_accessor(object, prop);
        }
    }
}

/// Collect class-level (`receiver == Class`) accessor properties — the
/// `class << self; attr_accessor :x` pairs — as camelCased name → type
/// (from the reader's RBS return / the writer's param). Instance accessors
/// are handled separately (they collapse to instance `var`s).
fn class_accessor_props(methods: &[MethodDef]) -> BTreeMap<String, Ty> {
    let mut props: BTreeMap<String, Ty> = BTreeMap::new();
    for m in methods {
        if m.receiver != MethodReceiver::Class {
            continue;
        }
        match m.kind {
            AccessorKind::AttributeReader => {
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    props.entry(camel(m.name.as_str())).or_insert_with(|| (**ret).clone());
                }
            }
            AccessorKind::AttributeWriter => {
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    if let Some(p) = params.first() {
                        let base = m.name.as_str().trim_end_matches('=');
                        props.entry(camel(base)).or_insert_with(|| p.ty.clone());
                    }
                }
            }
            AccessorKind::Method => {}
        }
    }
    props
}

/// Declaration for an object-level accessor property. A non-null reference
/// type (the global adapter slot) is `lateinit var` — set once at boot, so
/// a nullable default would force `!!` at every read; primitives/nullables
/// fall back to a defaulted `var`.
fn object_property_decl(name: &str, ty: &Ty) -> String {
    let kt = kotlin_ty(ty);
    if can_lateinit(ty) {
        format!("lateinit var {name}: {kt}")
    } else {
        format!("var {name}: {kt} = {}", default_for(ty))
    }
}

/// `lateinit` is legal only for non-null, non-primitive types.
fn can_lateinit(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Nil | Ty::Untyped | Ty::Var { .. } => false,
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => false,
        _ => true,
    }
}

/// True when any method references Ruby's class-name reflection — a bare
/// (implicit-self) `name` send with no args, as in `"#{name}.table_name
/// must be overridden"`.
fn references_class_name(methods: &[MethodDef]) -> bool {
    methods.iter().any(|m| sends_class_name(&m.body))
}

fn sends_class_name(e: &Expr) -> bool {
    let hit = matches!(
        &*e.node,
        ExprNode::Send { recv, method, args, .. }
            if method.as_str() == "name"
                && args.is_empty()
                && matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef))
    );
    hit || expr_children(e).iter().any(|c| sends_class_name(c))
}

/// True when a method body is empty (`def x; end` → an empty `Seq`).
fn is_empty_body(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

/// Infer types for body-only ivars (those without an `attr_*` accessor) so
/// they don't all collapse to `Any?`. Two signals, strongest first:
///   1. A pure reader method whose body *is* the ivar (`def errors;
///      @errors; end`) donates its declared return type — this is how
///      `@errors`/`@persisted`/`@destroyed` get `Array[String]`/`bool`
///      from `base.rbs` without an ivar-declaration syntax in RBS.
///   2. Otherwise, the literal an ivar is assigned (`@persisted = false`).
/// Same shape works for flash/session's `@data` once they're wired.
fn infer_body_ivar_types(methods: &[MethodDef]) -> BTreeMap<String, Ty> {
    let mut out: BTreeMap<String, Ty> = BTreeMap::new();

    // Signal 1: reader methods returning exactly an ivar.
    for m in methods {
        if let (Some(ivar), Some(Ty::Fn { ret, .. })) =
            (body_returns_ivar(&m.body), m.signature.as_ref())
        {
            if !matches!(&**ret, Ty::Nil) {
                out.entry(ivar).or_insert_with(|| (**ret).clone());
            }
        }
    }

    // Signal 1.5: a concrete `Ty` carried on an ivar read/assign node (the
    // typer often knows it — e.g. `@comments_cache` reads as
    // `Array[Comment]` from the has_many association) — for ivars not
    // already fixed by a reader.
    for m in methods {
        collect_ivar_node_types(&m.body, &mut out);
    }

    // Signal 2: literal assignments, only for ivars not already inferred.
    for m in methods {
        collect_ivar_literal_types(&m.body, &mut out);
    }

    out
}

/// Record the `Ty` the typer attached to an ivar read (`@x`) or to the
/// value of `@x = …`, when it's concrete (not `Untyped`/`Var`). Never
/// overwrites a stronger signal already present.
fn collect_ivar_node_types(e: &Expr, out: &mut BTreeMap<String, Ty>) {
    let useful = |ty: &Ty| !matches!(ty, Ty::Untyped | Ty::Var { .. } | Ty::Nil);
    match &*e.node {
        ExprNode::Ivar { name } => {
            if let Some(ty) = e.ty.as_ref().filter(|t| useful(t)) {
                out.entry(camel(name.as_str())).or_insert_with(|| ty.clone());
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            if let Some(ty) = value.ty.as_ref().filter(|t| useful(t)) {
                out.entry(camel(name.as_str())).or_insert_with(|| ty.clone());
            }
        }
        _ => {}
    }
    for child in expr_children(e) {
        collect_ivar_node_types(child, out);
    }
}

/// If a method body is (or ends in) a bare ivar read, return that ivar's
/// camel-cased name. Covers `@x`, `return @x`, and a `Seq` ending in `@x`.
fn body_returns_ivar(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Ivar { name } => Some(camel(name.as_str())),
        ExprNode::Return { value } => body_returns_ivar(value),
        ExprNode::Seq { exprs } => exprs.last().and_then(body_returns_ivar),
        _ => None,
    }
}

/// Record `@ivar = <literal>` types as a fallback, never overwriting a
/// type already established by a reader (signal 1 is stronger).
fn collect_ivar_literal_types(e: &Expr, out: &mut BTreeMap<String, Ty>) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*e.node {
        if let Some(ty) = literal_ty(value) {
            out.entry(camel(name.as_str())).or_insert(ty);
        }
    }
    for child in expr_children(e) {
        collect_ivar_literal_types(child, out);
    }
}

/// The `Ty` of a literal expression, when it's unambiguous from the node.
fn literal_ty(e: &Expr) -> Option<Ty> {
    use crate::expr::Literal;
    match &*e.node {
        ExprNode::Lit { value: Literal::Bool { .. } } => Some(Ty::Bool),
        ExprNode::Lit { value: Literal::Int { .. } } => Some(Ty::Int),
        ExprNode::Lit { value: Literal::Float { .. } } => Some(Ty::Float),
        ExprNode::Lit { value: Literal::Str { .. } } => Some(Ty::Str),
        _ => None,
    }
}

/// Default initializer for a property type (Kotlin requires properties
/// be initialized).
fn default_for(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Array { .. } => "mutableListOf()".to_string(),
        Ty::Hash { .. } => "mutableMapOf()".to_string(),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "null".to_string()
        }
        _ => "null".to_string(),
    }
}
