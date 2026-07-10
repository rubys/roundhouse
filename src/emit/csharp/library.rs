//! `LibraryClass` → C# file.
//!
//! Ported from `src/emit/kotlin/library.rs`, adapted to C#'s class shape:
//!   - Ruby `initialize` → a C# constructor (no `init` block); a
//!     `super(args)` becomes a `: base(args)` clause. A default `attrs = {}`
//!     param (not a compile-time constant in C#) becomes a nullable param
//!     coalesced in the body.
//!   - Instance `Method`s → methods; class methods (`def self.x`) → `static`
//!     methods (C# has no `companion object`).
//!   - Ruby `[]` / `[]=` collapse into a single C# indexer (`this[string …]`).
//!   - `attr_*` accessors collapse into auto-properties.
//!   - Ruby's implicit return becomes an explicit `return` on the final
//!     statement of value-returning methods.
//!
//! Phase 2 covers the model subset (models + `<Model>Row`/`<Model>Params`
//! siblings + the abstract `ApplicationRecord`). Method and public-property
//! names emit idiomatic PascalCase (`naming::pascal`); the `camel`-keyed
//! classification maps drive lookup only (see `expr.rs`).
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

use super::expr::{
    begin_method, emit_expr, emit_stmt, hoisted_decls, register_method_params, set_current_class,
    set_instance_prop_types, set_instance_props, set_ivar_renames, set_param_names,
    set_returns_unit,
};
use super::naming::{camel, pascal, pascal_of_camel, type_name};
use super::ty::csharp_ty;

/// The `using` + `namespace` header every emitted C# file carries.
const FILE_HEADER: &str = "using System;\n\
                           using System.Collections.Generic;\n\
                           using System.Linq;\n\
                           using System.Text;\n\
                           using System.Text.RegularExpressions;\n\n\
                           namespace Roundhouse;\n\n";

/// Emit a `LibraryClass` as a standalone C# file under `app/models/<Name>.cs`.
pub fn emit_class_file(lc: &LibraryClass) -> EmittedFile {
    emit_class_file_in(lc, "app/models")
}

/// Emit a `LibraryClass` under `<dir>/<Name>.cs` (controllers route to
/// `app/controllers`, origin-tagged Params siblings to `app/models`).
pub fn emit_class_file_in(lc: &LibraryClass, dir: &str) -> EmittedFile {
    let name = lc.name.0.as_str();
    let last = name.rsplit("::").next().unwrap_or(name);
    EmittedFile {
        path: PathBuf::from(format!("{dir}/{last}.cs")),
        content: format!("{FILE_HEADER}{}", emit_library_class(lc)),
    }
}

/// Emit a module of free functions (one `module_path`, e.g. `RouteHelpers`'
/// `article_path(id)`) as a C# `static class` under `app/runtime/<Name>.cs`.
/// They're class-method-shaped (no instance state), so they reuse the module
/// (`static class`) emit path.
pub fn emit_function_module(funcs: &[crate::dialect::LibraryFunction]) -> Option<EmittedFile> {
    let first = funcs.first()?;
    let name = first.module_path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
    let enclosing = first.module_path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            block_param: None,
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(crate::ident::Symbol::from(enclosing.clone())),
            kind: AccessorKind::Method,
            is_async: f.is_async,
            mutates_self: false,
        })
        .collect();
    let content = emit_module(&methods).ok()?;
    Some(EmittedFile {
        path: PathBuf::from(format!("app/runtime/{name}.cs")),
        content: format!("{FILE_HEADER}{content}"),
    })
}

pub fn emit_library_class_result(lc: &LibraryClass) -> Result<String, String> {
    Ok(emit_library_class(lc))
}

/// Render a module-level constant (`ESCAPES = {...}`) as a fragment of the
/// shared `partial class RuntimeConstants` — C# has no top-level constant, so
/// the runtime files reach these via `using static Roundhouse.RuntimeConstants`
/// (added to the runtime `module_prelude`). The type is inferred from the
/// literal so the field is precisely typed (`Dictionary<string,string>` for a
/// string→string hash), and the RHS is target-typed `new()` to match.
pub fn emit_module_constant(name: &str, value: &Expr) -> String {
    let (ty, rhs) = match &*value.node {
        ExprNode::Hash { entries, .. } if !entries.is_empty() => {
            let vty = homogeneous_lit_type(entries.iter().map(|(_, v)| v));
            let pairs: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("[{}] = {}", emit_expr(k), emit_expr(v)))
                .collect();
            (format!("Dictionary<string, {vty}>"), format!("new() {{ {} }}", pairs.join(", ")))
        }
        ExprNode::Array { elements, .. } if !elements.is_empty() => {
            let ety = homogeneous_lit_type(elements.iter());
            let els: Vec<String> = elements.iter().map(emit_expr).collect();
            (format!("List<{ety}>"), format!("new() {{ {} }}", els.join(", ")))
        }
        ExprNode::Lit { value: Literal::Regex { .. } } => ("Regex".to_string(), emit_expr(value)),
        _ => {
            let ty = value.ty.as_ref().map(csharp_ty).unwrap_or_else(|| "object?".to_string());
            (ty, emit_expr(value))
        }
    };
    format!("public static partial class RuntimeConstants {{ public static readonly {ty} {name} = {rhs}; }}")
}

/// The C# element type for a homogeneous-literal collection — `long`/`double`/
/// `bool`/`string` when every element is that literal kind, else `object?`.
fn homogeneous_lit_type<'a>(mut elems: impl Iterator<Item = &'a Expr>) -> &'static str {
    let kind = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Int { .. } } => Some("long"),
        ExprNode::Lit { value: Literal::Float { .. } } => Some("double"),
        ExprNode::Lit { value: Literal::Bool { .. } } => Some("bool"),
        ExprNode::Lit { value: Literal::Str { .. } } => Some("string"),
        _ => None,
    };
    match elems.next().and_then(kind) {
        Some(first) if elems.all(|e| kind(e) == Some(first)) => first,
        _ => "object?",
    }
}

/// Render a Ruby `module X` (a set of class methods) as a C# `static class`.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    set_instance_prop_types(std::collections::HashMap::new());
    super::expr::set_object_tl_fields(HashSet::new());
    set_instance_props(HashSet::new());
    set_ivar_renames(std::collections::HashMap::new());
    let name = methods
        .first()
        .and_then(|m| m.enclosing_class.as_ref())
        .map(|s| type_name(s.as_str()))
        .unwrap_or_default();
    // `self` in a module function is the static class — set it so a sibling
    // call (`Router.match` → `self.match_pattern`) resolves to `Router.…`.
    set_current_class(&name);
    register_params_for(&name, methods);
    // `partial` so a hand-written C# runtime file can add native overloads to a
    // transpiled module (`JsonBuilder.EncodeDatetime(DateTimeOffset?)` in
    // runtime/csharp/RhDateTime.cs). Harmless for modules never reopened.
    let mut out = format!("public static partial class {name} {{\n");
    for m in methods {
        out.push_str(&indent_method(&emit_method(m, "static ")));
        out.push('\n');
    }
    out.push_str("}\n");
    Ok(out)
}

pub fn emit_library_class(lc: &LibraryClass) -> String {
    let name = lc.name.0.as_str();
    let class_name = type_name(name);
    register_params_for(&class_name, &lc.methods);

    if lc.is_module {
        return emit_static_class(lc, &class_name);
    }

    super::expr::set_object_tl_fields(HashSet::new());

    // Temporal (Date/DateTime/Time) columns: a reader whose return type is
    // `DateTimeOffset` (`Ty::Time`). These must NOT collapse into an
    // auto-property — C# couples a property's get/set to one type, but the
    // reader's body parses the `<col>_raw` String storage (an ordinary
    // accessor pair from the shared lowering, collapsing to a normal
    // `CreatedAtRaw { get; set; }` auto-property) and yields
    // `DateTimeOffset?`. The reader emits as an explicit computed getter
    // rendering that body. Keyed by `camel` name.
    let temporal_readers: Vec<&crate::dialect::MethodDef> = lc
        .methods
        .iter()
        .filter(|m| {
            m.kind == AccessorKind::AttributeReader && signature_ret_is_time(m.signature.as_ref())
        })
        .collect();
    let temporal_cols: HashSet<String> = temporal_readers
        .iter()
        .map(|m| camel(m.name.as_str()))
        .collect();

    // 1. Accessor-derived properties (name → type). Temporal readers are
    // excluded — they emit as explicit `DateTimeOffset?` getters below
    // instead of a `{ get; set; }` auto-property.
    let mut prop_types: BTreeMap<String, Ty> = BTreeMap::new();
    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                if temporal_cols.contains(&camel(m.name.as_str())) {
                    continue;
                }
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    prop_types.entry(camel(m.name.as_str())).or_insert_with(|| (**ret).clone());
                }
            }
            AccessorKind::AttributeWriter => {
                let base = m.name.as_str().trim_end_matches('=');
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    if let Some(p) = params.first() {
                        prop_types.entry(camel(base)).or_insert_with(|| p.ty.clone());
                    }
                }
            }
            AccessorKind::Method => {}
        }
    }

    // 2. Body-only ivars.
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }

    let init = lc
        .methods
        .iter()
        .find(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "initialize");

    // Ivars assigned in `initialize` — their properties are non-null after
    // construction, so a reference-typed one defaults to `null!` (the C#
    // "set in the ctor" marker) rather than going nullable.
    let init_assigned: HashSet<String> = {
        let mut m: BTreeMap<String, ()> = BTreeMap::new();
        if let Some(i) = init {
            collect_ivars(&i.body, &mut m);
        }
        m.into_keys().collect()
    };

    let parent_name = lc.parent.as_ref().map(|p| {
        let last = type_name(p.0.as_str());
        match last.as_str() {
            "StandardError" | "RuntimeError" => "Exception".to_string(),
            other => other.to_string(),
        }
    });

    let inherited: HashSet<String> = lc
        .parent
        .as_ref()
        .map(|p| type_name(p.0.as_str()))
        .map(|p| super::expr::ancestor_members(&p))
        .unwrap_or_default();
    let inherited_props: HashSet<String> = lc
        .parent
        .as_ref()
        .map(|p| type_name(p.0.as_str()))
        .map(|p| super::expr::ancestor_props(&p))
        .unwrap_or_default();

    let parent_clause = match &parent_name {
        Some(pn) => format!(" : {pn}"),
        None => String::new(),
    };
    let mut out = format!("public class {class_name}{parent_clause} {{\n");

    // Properties (auto-properties). Constructor-param-backed properties are
    // assigned in the constructor; column props are plain `public` (leaf, not
    // overridden). Inherited slots (e.g. `id` from the base) are skipped.
    let ctor_param_names: HashSet<String> = init
        .map(|m| m.params.iter().map(|p| camel(p.name.as_str())).collect())
        .unwrap_or_default();
    for (n, ty) in &prop_types {
        if inherited_props.contains(n) && !ctor_param_names.contains(n) {
            continue;
        }
        out.push_str(&format!(
            "    {}\n",
            render_member_full("public", &pascal_of_camel(n), ty, init_assigned.contains(n))
        ));
    }
    // A body ivar whose name collides with a same-named instance method
    // (`@errors` + `def errors`) can't share that name in C#, so the ivar
    // emits as a private renamed field (`_errors`) that every `@ivar`
    // reference rewrites to. The method keeps the public name.
    let methods_set = instance_method_names(lc);
    let ivar_renames: std::collections::HashMap<String, String> = body_ivars
        .keys()
        .filter(|n| methods_set.contains(*n))
        .map(|n| (n.clone(), format!("_{n}")))
        .collect();
    set_ivar_renames(ivar_renames.clone());

    let inferred_ivar_types = infer_body_ivar_types(&lc.methods);
    for n in body_ivars.keys() {
        if !prop_types.contains_key(n) && !inherited_props.contains(n) {
            let (vis, field) = match ivar_renames.get(n) {
                Some(renamed) => ("private", renamed.clone()),
                None => ("public", pascal_of_camel(n)),
            };
            match inferred_ivar_types.get(n) {
                Some(ty) => {
                    out.push_str(&format!("    {}\n", render_member_vis(vis, &field, ty)))
                }
                None => out.push_str(&format!("    {vis} object? {field} = null;\n")),
            }
        }
    }
    // Temporal columns: the explicit `DateTimeOffset?` reader getter,
    // rendering the IR body (`parse_db_time(@<col>_raw)` →
    // `Roundhouse.RhDateTime.Parse(CreatedAtRaw)`; the storage property
    // itself collapsed with the ordinary accessors above). Emitted in
    // stable (name-sorted) order.
    let mut temporal_sorted = temporal_readers.clone();
    temporal_sorted.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for m in &temporal_sorted {
        let getter = pascal_of_camel(&camel(m.name.as_str()));
        let ret = match m.signature.as_ref() {
            Some(Ty::Fn { ret, .. }) => (**ret).clone(),
            _ => Ty::Time,
        };
        out.push_str(&format!(
            "    public {} {getter} => {};\n",
            csharp_ty(&ret),
            super::expr::emit_expr(&m.body),
        ));
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() || !temporal_cols.is_empty() {
        out.push('\n');
    }

    // Property/type registries for body emit. Renamed (private) ivars are
    // excluded — a self-send of that name resolves to the method, not a prop.
    let instance_props: HashSet<String> = prop_types
        .keys()
        .chain(body_ivars.keys())
        .chain(inherited_props.iter())
        .filter(|n| !ivar_renames.contains_key(*n))
        .cloned()
        .collect();
    set_instance_props(instance_props);
    let mut prop_ty_map: std::collections::HashMap<String, Ty> =
        prop_types.iter().map(|(n, t)| (n.clone(), t.clone())).collect();
    for (n, t) in &inferred_ivar_types {
        prop_ty_map.entry(n.clone()).or_insert_with(|| t.clone());
    }
    set_instance_prop_types(prop_ty_map);
    set_current_class(&class_name);

    // Constructor (from `initialize`).
    if let Some(m) = init {
        out.push_str(&indent_method(&emit_constructor(&class_name, m)));
        out.push('\n');
    }

    let member_modifier = |name: &str| -> &'static str {
        if inherited.contains(name) {
            "override "
        } else {
            "virtual "
        }
    };

    // Instance methods (skip accessors, initialize, and []/[]= — handled as an
    // indexer below).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance
            && m.kind == AccessorKind::Method
            && m.name.as_str() != "initialize"
            && !matches!(m.name.as_str(), "[]" | "[]=")
        {
            out.push_str(&indent_method(&emit_method(m, member_modifier(&member_name(m)))));
            out.push('\n');
        }
    }

    // Indexer: merge `[]` (get) and `[]=` (set) into one C# indexer.
    let getter = lc.methods.iter().find(|m| {
        m.receiver == MethodReceiver::Instance && m.name.as_str() == "[]"
    });
    let setter = lc.methods.iter().find(|m| {
        m.receiver == MethodReceiver::Instance && m.name.as_str() == "[]="
    });
    if getter.is_some() || setter.is_some() {
        let modifier = if inherited.contains("get") || inherited.contains("set") {
            "override "
        } else {
            "virtual "
        };
        out.push_str(&indent_method(&emit_indexer(modifier, getter, setter)));
        out.push('\n');
    }

    // Polymorphic `schemaColumns` (virtual instance shadow of the static
    // column list) — see the Kotlin emitter's note.
    if lc.methods.iter().any(|m| {
        m.receiver == MethodReceiver::Class && m.name.as_str() == "schema_columns"
    }) {
        let (modifier, body) = if lc.parent.is_none() {
            (
                "virtual",
                "throw new NotImplementedException(\"ActiveRecord::Base.schema_columns must be overridden\");".to_string(),
            )
        } else {
            ("override", format!("return {class_name}.SchemaColumnsList();"))
        };
        out.push_str(&format!(
            "    public {modifier} List<string> SchemaColumns() {{ {body} }}\n\n"
        ));
    }

    // Class methods → `static` methods.
    let class_methods: Vec<&MethodDef> = lc
        .methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .collect();
    let needs_name = references_class_name(&lc.methods)
        && !lc.methods.iter().any(|m| m.name.as_str() == "name");
    if needs_name {
        out.push_str(&format!(
            "    public static string Name() {{\n        return {:?};\n    }}\n",
            lc.name.0.as_str()
        ));
    }
    // C# forbids a static member sharing a name with an instance member. A
    // class method colliding with an instance accessor/method (e.g.
    // `ApplicationRecord`'s `abstract` marker) is dropped — these markers are
    // never called on a concrete model. (`schema_columns` is exempt: it's
    // renamed to `schemaColumnsList`, so it doesn't collide.)
    let mut inst_members = instance_member_names(lc);
    inst_members.extend(prop_types.keys().cloned());
    inst_members.extend(body_ivars.keys().cloned());
    // Static methods that shadow an ancestor's static (`Article.find` over
    // `Base.find`) take `new` (C# statics don't override — CS0108 otherwise).
    let ancestor_statics = super::expr::ancestor_static_methods(&class_name);
    for m in class_methods.iter() {
        if m.name.as_str() != "schema_columns" && inst_members.contains(&member_name(m)) {
            continue;
        }
        let modifier = if ancestor_statics.contains(&class_method_emitted_name(m)) {
            "new static "
        } else {
            "static "
        };
        out.push_str(&indent_method(&emit_method(m, modifier)));
        out.push('\n');
    }
    let present: HashSet<String> = class_methods.iter().map(|m| member_name(m)).collect();
    out.push_str(&synth_inherited_finders(&class_name, &present));

    out.push_str("}\n");
    out
}

/// A Ruby `module` → C# `static class`. Class-level `attr_accessor` (from
/// `class << self`) collapses to a static property.
fn emit_static_class(lc: &LibraryClass, class_name: &str) -> String {
    set_instance_prop_types(std::collections::HashMap::new());
    // `self` in a module function is the static class itself.
    set_current_class(class_name);
    super::expr::set_object_tl_fields(HashSet::new());
    set_instance_props(HashSet::new());
    set_ivar_renames(std::collections::HashMap::new());
    let accessor_props = class_accessor_props(&lc.methods);
    // `partial` so a hand-written C# runtime file can add native overloads to a
    // transpiled module (e.g. `JsonBuilder.EncodeDatetime(DateTimeOffset?)` in
    // runtime/csharp/RhDateTime.cs). Harmless for modules never reopened.
    let mut out = format!("public static partial class {class_name} {{\n");
    for (n, ty) in &accessor_props {
        // Public static accessor (e.g. `ActiveRecord.Adapter`) → PascalCase to
        // match the references emitted at read sites; `n` stays the camel key.
        let pn = pascal_of_camel(n);
        let cs = csharp_ty(ty);
        // The `ActiveRecord.adapter` global is never assigned for C# (models go
        // Db-direct). Default it to a throwing `NullAdapter` so it stays
        // non-null (the dead Base defaults that read it compile without
        // nullable-deref warnings, and throw if ever actually hit).
        if cs == "AdapterInterface" {
            out.push_str(&format!("    public static {cs} {pn} = new NullAdapter();\n"));
            continue;
        }
        // Other class-type slots defaulting to null need a nullable type.
        let default = default_for(ty);
        let cs = if default == "null" && !cs.ends_with('?') { format!("{cs}?") } else { cs };
        out.push_str(&format!("    public static {cs} {pn} = {default};\n"));
    }

    // Module-level `@ivar` state (ViewHelpers' `@slots` content_for store) →
    // thread-local static fields: a concurrent server (Kestrel) would
    // otherwise bleed singleton state across requests. Reads/writes route
    // through `.Value` (OBJECT_TL_FIELDS). Excludes anything already declared
    // as a class accessor.
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }
    let body_ivars: BTreeMap<String, ()> =
        body_ivars.into_iter().filter(|(n, _)| !accessor_props.contains_key(n)).collect();
    if !body_ivars.is_empty() {
        let inferred = infer_body_ivar_types(&lc.methods);
        super::expr::set_object_tl_fields(body_ivars.keys().cloned().collect());
        set_instance_props(body_ivars.keys().cloned().collect());
        for n in body_ivars.keys() {
            let (ty, init) = match inferred.get(n) {
                // A module store's hash values are nullable — `@slots` is read
                // via `fetch(slot, nil)`, so the value type must admit null.
                Some(Ty::Hash { key, value }) => {
                    let vt = csharp_ty(value);
                    let vt = if vt.ends_with('?') { vt } else { format!("{vt}?") };
                    (format!("Dictionary<{}, {vt}>", csharp_ty(key)), "new()".to_string())
                }
                Some(t) => (csharp_ty(t), default_for(t)),
                // An untyped module ivar assigned `{}` defaults to a dict.
                None => ("Dictionary<string, object?>".to_string(), "new()".to_string()),
            };
            out.push_str(&format!(
                "    private static readonly ThreadLocal<{ty}> {n} = new(() => {init});\n"
            ));
        }
    }
    if !accessor_props.is_empty() {
        out.push('\n');
    }
    for m in &lc.methods {
        if m.kind == AccessorKind::Method {
            out.push_str(&indent_method(&emit_method(m, "static ")));
            out.push('\n');
        }
    }
    out.push_str("}\n");
    out
}

/// Per-model copies of the public AR class methods Base defines, delegating
/// to the model's own `_adapter_*` static members. Emitted (indented) only
/// for a Base-subclass model (the `_adapterAll` marker) and only for finders
/// the class doesn't already define.
fn synth_inherited_finders(t: &str, present: &HashSet<String>) -> String {
    if !present.contains("_adapterAll") {
        return String::new();
    }
    let mut out = String::new();
    // `name` is the camel-keyed existence check (matched against `present`, the
    // class's camelCase member names); the body renders PascalCase definitions
    // and calls into the model's own PascalCase `_Adapter*` statics.
    let mut emit = |name: &str, body: String| {
        if !present.contains(name) {
            out.push_str(&indent_method(&body));
            out.push('\n');
        }
    };
    emit("all", format!("public new static List<{t}> All() {{\n    return _AdapterAll();\n}}\n"));
    emit(
        "find",
        format!(
            "public new static {t} Find(long id) {{\n    var result = _AdapterFindById(id);\n    if (result == null) {{\n        throw new RecordNotFound($\"Couldn't find {t} with id={{id}}\");\n    }}\n    return result;\n}}\n"
        ),
    );
    emit("count", "public new static long Count() {\n    return _AdapterCount();\n}\n".to_string());
    emit(
        "existsPred",
        "public new static bool ExistsPred(long id) {\n    return _AdapterExistsByIdPred(id);\n}\n".to_string(),
    );
    emit(
        "last",
        format!(
            "public new static {t}? Last() {{\n    return _AdapterLast();\n}}\n"
        ),
    );
    emit(
        "destroyAll",
        format!(
            "public new static List<{t}> DestroyAll() {{\n    var records = All();\n    foreach (var it in records) {{ it.Destroy(); }}\n    return records;\n}}\n"
        ),
    );
    emit(
        "create",
        format!(
            "public new static {t} Create(Dictionary<string, object?>? attrs = null) {{\n    attrs ??= new Dictionary<string, object?>();\n    var instance = new {t}(attrs);\n    instance.Save();\n    return instance;\n}}\n"
        ),
    );
    emit(
        "createBang",
        format!(
            "public new static {t} CreateBang(Dictionary<string, object?>? attrs = null) {{\n    attrs ??= new Dictionary<string, object?>();\n    var instance = new {t}(attrs);\n    if (!instance.Save()) {{\n        throw new RecordInvalid(instance);\n    }}\n    return instance;\n}}\n"
        ),
    );
    out
}

fn register_params_for(receiver: &str, methods: &[MethodDef]) {
    for m in methods {
        register_method_params(
            receiver,
            m.name.as_str(),
            m.params.iter().map(|p| camel(p.name.as_str())).collect(),
        );
    }
}

fn indent_method(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn member_name(m: &MethodDef) -> String {
    match m.name.as_str() {
        "[]" => "get".to_string(),
        "[]=" => "set".to_string(),
        _ => camel(m.name.as_str()),
    }
}

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

pub fn register_class_hierarchy(classes: &[LibraryClass]) {
    for lc in classes {
        if lc.is_module {
            continue;
        }
        let name = type_name(lc.name.0.as_str());
        let parent = lc.parent.as_ref().map(|p| type_name(p.0.as_str()));
        super::expr::register_class_hierarchy(&name, parent.as_deref(), instance_member_names(lc));
        super::expr::register_instance_methods(&name, instance_method_names(lc));
        let statics: HashSet<String> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class)
            .map(class_method_emitted_name)
            .collect();
        super::expr::register_static_methods(&name, statics);
    }
}

/// The C# name a class (static) method emits under — `schema_columns` is
/// renamed to avoid colliding with the virtual instance `schemaColumns()`.
fn class_method_emitted_name(m: &MethodDef) -> String {
    if m.name.as_str() == "schema_columns" {
        "schemaColumnsList".to_string()
    } else {
        camel(m.name.as_str())
    }
}

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

/// Emit a constructor from the Ruby `initialize`. A `super(args)` becomes a
/// `: base(args)` clause; a default `attrs = {}` (non-constant in C#) becomes
/// a nullable param coalesced at the top of the body.
fn emit_constructor(class_name: &str, m: &MethodDef) -> String {
    let (params, prelude) = render_params(m);
    let super_args = find_super_args(&m.body);
    let base_clause = match super_args {
        Some(args) => format!(" : base({})", args.join(", ")),
        None => String::new(),
    };

    begin_method(&m.body);
    set_param_names(m.params.iter().map(|p| camel(p.name.as_str())).collect());
    set_returns_unit(true);
    super::expr::set_in_static(false);
    // Drop the `super(...)` call from the body (it's in the base clause) and
    // render the rest as statements.
    let body = emit_body_no_super(&m.body);
    let hoist = hoisted_decls();
    let mut lines: Vec<String> = Vec::new();
    lines.extend(prelude);
    lines.extend(hoist);
    if !body.is_empty() {
        lines.push(body);
    }
    let body = lines.join("\n");

    format!(
        "public {class_name}({}){base_clause} {{\n{}\n}}\n",
        params.join(", "),
        indent4(&body)
    )
}

/// Emit a C# indexer from the lowered `[]`/`[]=` methods.
fn emit_indexer(modifier: &str, getter: Option<&MethodDef>, setter: Option<&MethodDef>) -> String {
    let key = getter
        .or(setter)
        .and_then(|m| m.params.first())
        .map(|p| camel(p.name.as_str()))
        .unwrap_or_else(|| "name".to_string());
    // Key + element types. An `override` indexer must match the base's
    // `object? this[string]`; a class defining its own (Flash → `string?`
    // values over `untyped` keys, Session → `object?`) takes them from its
    // own signature (getter return = element type, first param = key type).
    let is_override = modifier.contains("override");
    let key_ty = if is_override {
        "string".to_string()
    } else {
        getter
            .or(setter)
            .and_then(|m| match m.signature.as_ref() {
                Some(Ty::Fn { params, .. }) => params.first().map(|p| csharp_ty(&p.ty)),
                _ => None,
            })
            .unwrap_or_else(|| "string".to_string())
    };
    let element_ty = if is_override {
        "object?".to_string()
    } else {
        getter
            .and_then(|m| match m.signature.as_ref() {
                Some(Ty::Fn { ret, .. }) => Some(csharp_ty(ret)),
                _ => None,
            })
            .unwrap_or_else(|| "object?".to_string())
    };

    super::expr::set_in_static(false);
    let mut accessors = String::new();
    if let Some(g) = getter {
        begin_method(&g.body);
        set_param_names(g.params.iter().map(|p| camel(p.name.as_str())).collect());
        set_returns_unit(false);
        let body = emit_body(&g.body, true);
        let hoist = hoisted_decls();
        let body = prepend_hoist(hoist, body);
        accessors.push_str(&format!("    get {{\n{}\n    }}\n", indent4(&indent4(&body))));
    }
    if let Some(s) = setter {
        begin_method(&s.body);
        // The setter's second param is the value; C# exposes it as `value`.
        // Bind the lowered value-param name to `value`.
        let mut pnames: HashSet<String> = s.params.iter().map(|p| camel(p.name.as_str())).collect();
        pnames.insert("value".to_string());
        set_param_names(pnames);
        set_returns_unit(true);
        let value_param = s.params.get(1).map(|p| camel(p.name.as_str()));
        let body = emit_body(&s.body, false);
        // Rename the lowered value param to C#'s implicit `value`.
        let body = match value_param {
            Some(vp) if vp != "value" => rename_ident(&body, &vp, "value"),
            _ => body,
        };
        let hoist = hoisted_decls();
        let body = prepend_hoist(hoist, body);
        accessors.push_str(&format!("    set {{\n{}\n    }}\n", indent4(&indent4(&body))));
    }
    format!("public {modifier}{element_ty} this[{key_ty} {key}] {{\n{accessors}}}\n")
}

/// Crude whole-word identifier rename (setter value-param → `value`). The
/// emitted body only references the param as a bare word, so a token-boundary
/// replace is safe here.
fn rename_ident(body: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
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

fn prepend_hoist(hoist: Vec<String>, body: String) -> String {
    if hoist.is_empty() {
        body
    } else {
        format!("{}\n{}", hoist.join("\n"), body)
    }
}

fn emit_method(m: &MethodDef, modifier: &str) -> String {
    // The static `schema_columns` column-list method would collide with the
    // synthesized virtual instance `schemaColumns()` (C# forbids same-name
    // static + instance members), so the static is renamed.
    let name = if m.receiver == MethodReceiver::Class && m.name.as_str() == "schema_columns" {
        "SchemaColumnsList".to_string()
    } else {
        pascal(m.name.as_str())
    };
    let (mut params, prelude) = render_params(m);

    // A `yield`ing method takes an explicit block parameter.
    if body_has_yield(&m.body) {
        let bt = match m.signature.as_ref() {
            Some(Ty::Fn { block: Some(b), .. }) => csharp_ty(b),
            _ => "Action<object?>".to_string(),
        };
        params.push(format!("{bt} block"));
    }

    let ret_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let returns_value = matches!(&ret_ty, Some(t) if !matches!(t, Ty::Nil));
    let ret_decl = match &ret_ty {
        Some(t) if !matches!(t, Ty::Nil) => csharp_ty(t),
        _ => "void".to_string(),
    };

    begin_method(&m.body);
    set_param_names(m.params.iter().map(|p| camel(p.name.as_str())).collect());
    set_returns_unit(!returns_value);
    // `self` in a static method is the class — render `self.x()` as
    // `Class.x()`, not `this.x()` (illegal in a C# static member).
    super::expr::set_in_static(modifier.contains("static"));

    let body = if returns_value && is_empty_body(&m.body) {
        let ret = ret_ty.clone().unwrap_or(Ty::Untyped);
        format!("return {};", default_for(&ret))
    } else {
        emit_body(&m.body, returns_value)
    };
    let hoist = hoisted_decls();
    let mut lines: Vec<String> = Vec::new();
    lines.extend(prelude);
    lines.extend(hoist);
    if !body.is_empty() {
        lines.push(body);
    }
    let body = lines.join("\n");

    format!(
        "public {modifier}{ret_decl} {name}({}) {{\n{}\n}}\n",
        params.join(", "),
        indent4(&body)
    )
}

/// Render a method's params (always typed) plus any coalesce-prelude lines for
/// non-constant defaults (`attrs = {}` → `attrs ??= new …()`).
fn render_params(m: &MethodDef) -> (Vec<String>, Vec<String>) {
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    };
    let mut decls = Vec::new();
    let mut prelude = Vec::new();
    for (i, p) in m.params.iter().enumerate() {
        let pn = camel(p.name.as_str());
        let ty = sig_params
            .and_then(|sp| sp.get(i))
            .map(|sp| csharp_ty(&sp.ty))
            .unwrap_or_else(|| "object?".to_string());
        match &p.default {
            Some(d) if is_empty_container(d) => {
                let ctor = match &*d.node {
                    ExprNode::Hash { .. } => container_ctor(&ty, "Dictionary<string, object?>"),
                    _ => container_ctor(&ty, "List<object?>"),
                };
                let nty = if ty.ends_with('?') { ty.clone() } else { format!("{ty}?") };
                decls.push(format!("{nty} {pn} = null"));
                prelude.push(format!("{pn} ??= {ctor};"));
            }
            Some(d) if is_constant(d) => {
                decls.push(format!("{ty} {pn} = {}", emit_expr(d)));
            }
            Some(d) => {
                // Non-constant, non-container default: fall back to nullable +
                // coalesce so it stays a legal C# optional param.
                let nty = if ty.ends_with('?') { ty.clone() } else { format!("{ty}?") };
                decls.push(format!("{nty} {pn} = null"));
                prelude.push(format!("{pn} ??= {};", emit_expr(d)));
            }
            None => decls.push(format!("{ty} {pn}")),
        }
    }
    (decls, prelude)
}

fn container_ctor(ty: &str, fallback: &str) -> String {
    let base = ty.trim_end_matches('?');
    if base.starts_with("Dictionary<") || base.starts_with("List<") {
        format!("new {base}()")
    } else {
        format!("new {fallback}()")
    }
}

fn is_empty_container(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Hash { entries, .. } if entries.is_empty())
        || matches!(&*e.node, ExprNode::Array { elements, .. } if elements.is_empty())
}

fn is_constant(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { .. })
}

fn indent4(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit a method body as statements, adding an explicit `return` to the final
/// statement when the method returns a value.
fn emit_body(body: &Expr, returns_value: bool) -> String {
    if !returns_value {
        return emit_stmt(body);
    }
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> =
                exprs[..exprs.len() - 1].iter().map(emit_stmt).filter(|s| !s.is_empty()).collect();
            lines.push(wrap_return(&exprs[exprs.len() - 1]));
            lines.join("\n")
        }
        _ => wrap_return(body),
    }
}

/// Like `emit_body` (statement form) but drops a top-level `super(...)` call
/// (it lives in the constructor's `: base(...)` clause).
fn emit_body_no_super(body: &Expr) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .filter(|e| !matches!(&*e.node, ExprNode::Super { .. }))
            .map(emit_stmt)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::Super { .. } => String::new(),
        _ => emit_stmt(body),
    }
}

/// Render the final statement of a value-returning method with `return`.
fn wrap_return(e: &Expr) -> String {
    if let ExprNode::Seq { exprs } = &*e.node {
        if !exprs.is_empty() {
            return emit_body(e, true);
        }
    }
    // Hash/array literals in return position → target-typed `new()` so the
    // declared return type drives element types (`toH()` →
    // `Dictionary<string,string>`, not the literal's `<string,object?>`).
    match &*e.node {
        ExprNode::Hash { entries, .. } if entries.is_empty() => return "return new();".to_string(),
        ExprNode::Array { elements, .. } if elements.is_empty() => {
            return "return new();".to_string()
        }
        ExprNode::Hash { entries, .. } => {
            let pairs: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("[{}] = {}", emit_expr(k), emit_expr(v)))
                .collect();
            return format!("return new() {{ {} }};", pairs.join(", "));
        }
        ExprNode::Array { elements, .. } => {
            let els: Vec<String> = elements.iter().map(emit_expr).collect();
            return format!("return new() {{ {} }};", els.join(", "));
        }
        // A value-position `if` → return from each branch (C# has no
        // block-valued `if`). Empty branches return the type default.
        ExprNode::If { cond, then_branch, else_branch } => {
            let c = emit_expr(cond);
            let then = if branch_is_empty(then_branch) {
                "return default;".to_string()
            } else {
                wrap_return(then_branch)
            };
            let els = if branch_is_empty(else_branch) {
                "return default;".to_string()
            } else {
                wrap_return(else_branch)
            };
            return format!(
                "if ({c}) {{\n{}\n}} else {{\n{}\n}}",
                indent4(&then),
                indent4(&els)
            );
        }
        _ => {}
    }
    // Terminal statements take no `return` prefix.
    let is_raise_send = matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "raise"
    );
    let terminal = is_raise_send
        || matches!(
            &*e.node,
            ExprNode::Return { .. }
                | ExprNode::Raise { .. }
                | ExprNode::While { .. }
                | ExprNode::Assign { .. }
                | ExprNode::OpAssign { .. }
                | ExprNode::Super { .. }
                | ExprNode::Next { .. }
                | ExprNode::Break { .. }
                | ExprNode::If { .. }
        );
    if terminal {
        emit_stmt(e)
    } else {
        format!("return {};", emit_expr(e))
    }
}

fn branch_is_empty(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

fn body_has_yield(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Yield { .. }) || expr_children(e).iter().any(|c| body_has_yield(c))
}

fn find_super_args(e: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Super { args } = &*e.node {
        return Some(
            args.as_ref().map(|a| a.iter().map(emit_expr).collect()).unwrap_or_default(),
        );
    }
    for c in expr_children(e) {
        if let Some(r) = find_super_args(c) {
            return Some(r);
        }
    }
    None
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

fn is_empty_body(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

fn infer_body_ivar_types(methods: &[MethodDef]) -> BTreeMap<String, Ty> {
    let mut out: BTreeMap<String, Ty> = BTreeMap::new();
    for m in methods {
        if let (Some(ivar), Some(Ty::Fn { ret, .. })) =
            (body_returns_ivar(&m.body), m.signature.as_ref())
        {
            if !matches!(&**ret, Ty::Nil) {
                out.entry(ivar).or_insert_with(|| (**ret).clone());
            }
        }
    }
    for m in methods {
        collect_ivar_node_types(&m.body, &mut out);
    }
    for m in methods {
        collect_ivar_literal_types(&m.body, &mut out);
    }
    out
}

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

fn body_returns_ivar(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Ivar { name } => Some(camel(name.as_str())),
        ExprNode::Return { value } => body_returns_ivar(value),
        ExprNode::Seq { exprs } => exprs.last().and_then(body_returns_ivar),
        _ => None,
    }
}

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

fn literal_ty(e: &Expr) -> Option<Ty> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Bool { .. } } => Some(Ty::Bool),
        ExprNode::Lit { value: Literal::Int { .. } } => Some(Ty::Int),
        ExprNode::Lit { value: Literal::Float { .. } } => Some(Ty::Float),
        ExprNode::Lit { value: Literal::Str { .. } } => Some(Ty::Str),
        _ => None,
    }
}

/// True when a method's return type is `Ty::Time` (or a `Time | Nil` union) —
/// i.e. a synthesized temporal-column reader (see `synth_attr_reader`). Gates
/// the decoupled `string` backing + `DateTimeOffset?` getter emit.
fn signature_ret_is_time(sig: Option<&Ty>) -> bool {
    fn is_time(t: &Ty) -> bool {
        match t {
            Ty::Time => true,
            Ty::Union { variants } => variants.iter().any(is_time),
            _ => false,
        }
    }
    matches!(sig, Some(Ty::Fn { ret, .. }) if is_time(ret))
}

/// Render an auto-property with a default initializer (C# requires non-null
/// reference props be initialized).
fn render_member(name: &str, ty: &Ty) -> String {
    render_member_vis("public", name, ty)
}

fn render_member_vis(vis: &str, name: &str, ty: &Ty) -> String {
    render_member_full(vis, name, ty, false)
}

/// Render an auto-property. A reference-typed property defaulting to `null`
/// goes nullable (CS8625) — unless it's assigned in the constructor
/// (`init_assigned`), in which case it stays non-null with the `null!`
/// "set-in-ctor" marker so later non-null uses (`@flash[...]`) don't warn.
fn render_member_full(vis: &str, name: &str, ty: &Ty, init_assigned: bool) -> String {
    let cs = csharp_ty(ty);
    let default = default_for(ty);
    if default == "null" && !cs.ends_with('?') {
        if init_assigned {
            return format!("{vis} {cs} {name} {{ get; set; }} = null!;");
        }
        return format!("{vis} {cs}? {name} {{ get; set; }} = null;");
    }
    format!("{vis} {cs} {name} {{ get; set; }} = {default};")
}

fn default_for(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0L".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Array { elem } => format!("new List<{}>()", csharp_ty(elem)),
        Ty::Hash { key, value } => format!("new Dictionary<{}, {}>()", csharp_ty(key), csharp_ty(value)),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "null".to_string()
        }
        _ => "null".to_string(),
    }
}
