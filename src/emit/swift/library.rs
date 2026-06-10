//! `LibraryClass` → Swift file.
//!
//! Renders a lowered class to idiomatic Swift (ported from
//! `src/emit/kotlin/library.rs`):
//!   - `attr_reader`/`attr_writer` accessor methods collapse into Swift
//!     `var` properties (the property *is* the accessor); the synthetic
//!     getter/setter `MethodDef`s are dropped.
//!   - Instance `Method`s → `func`; class methods (`def self.x`) →
//!     `static func` members (no companion-object wrapper — and Swift
//!     statics ARE inherited, unlike Kotlin companions).
//!   - Ruby's implicit return becomes an explicit `return` on the final
//!     statement of value-returning methods.
//!   - Swift requires every parameter typed, so params take their
//!     signature type, falling back to `Any?`. Params are
//!     underscore-labeled (`_ x: T`) — the lowered IR calls
//!     positionally; named-arg call sites are the Phase 5 kwargs story.
//!   - No `open`/`override` modifiers yet: the whole emit is one module
//!     (`open` is cross-module-only in Swift), and `override` marking
//!     needs the class-hierarchy registry — the Phase 2.x cluster.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::expr::{begin_method, emit_expr, wrap_return};
use super::naming::{camel, type_name};
use super::ty::swift_ty;

/// Render a Ruby module (bare functions, no instances) as a Swift
/// caseless `enum` namespace of static funcs — the idiomatic spelling;
/// Swift has no `object` keyword (plan delta 2). The enum name comes
/// from the methods' enclosing class. Consumed by
/// `runtime_loader::swift_units` for `Mode::Module` entries.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    super::expr::set_instance_prop_types(std::collections::HashMap::new());
    super::expr::set_current_class("");
    let name = methods
        .first()
        .and_then(|m| m.enclosing_class.as_ref())
        .map(|s| type_name(s.as_str()))
        .unwrap_or_default();
    let mut out = format!("enum {name} {{\n");
    for m in methods {
        out.push_str(&indent_method(&emit_method(m, true)));
        out.push('\n');
    }
    out.push_str("}\n");
    Ok(out)
}

/// `Result`-shaped wrapper over `emit_library_class`, the signature the
/// runtime loader's `TargetEmit` expects for `Mode::Library` entries.
pub fn emit_library_class_result(lc: &LibraryClass) -> Result<String, String> {
    Ok(emit_library_class(lc))
}

/// Class-level accessor pairs (`class << self; attr_accessor :x`) —
/// name → type, taken from the reader's return (or writer's param).
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

/// Pre-register a class set's emit-relevant facts so call sites resolve
/// regardless of render order: instance-method names (property-vs-method),
/// parents (ancestor walks), Error conformance (raise classification),
/// throwing methods (`try` insertion), and module-level accessor
/// properties (paren-dropping reads). Called for the runtime entries
/// (via the swift_units transform) and the models; `swift::emit` resets
/// the registries once at start.
pub fn register_classes(lcs: &[LibraryClass]) {
    for lc in lcs {
        let cls = type_name(lc.name.0.as_str());
        // Pure readers collapse into properties, so they're NOT methods
        // at call sites; the merged subscript registers under a marker
        // key for override detection.
        let mut methods: std::collections::HashSet<String> = lc
            .methods
            .iter()
            .filter(|m| {
                m.receiver == MethodReceiver::Instance
                    && m.kind == AccessorKind::Method
                    && !matches!(m.name.as_str(), "[]" | "[]=")
                    && !is_pure_reader(m)
            })
            .map(|m| camel(m.name.as_str()))
            .collect();
        if lc.methods.iter().any(|m| matches!(m.name.as_str(), "[]" | "[]=")) {
            methods.insert("__subscript__".to_string());
        }
        super::expr::register_class_methods(cls.clone(), methods);
        super::expr::register_class_props(cls.clone(), instance_prop_names(lc));
        let mut statics: std::collections::HashMap<String, String> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class && m.kind == AccessorKind::Method)
            .map(|m| {
                let ret = match m_sig_ret(m) {
                    Some(t) if !matches!(t, Ty::Nil) => swift_ty(&t),
                    _ => String::new(),
                };
                (camel(m.name.as_str()), ret)
            })
            .collect();
        // The synthesized `class func name()` participates in override
        // resolution like a real method.
        if !lc.methods.iter().any(|m| m.name.as_str() == "name")
            && lc.methods.iter().any(|m| references_class_name(&m.body))
        {
            statics.insert("name".to_string(), "String".to_string());
        }
        super::expr::register_static_methods(cls.clone(), statics);
        if let Some(p) = &lc.parent {
            let pn = p.0.as_str().rsplit("::").next().unwrap_or("");
            if matches!(pn, "StandardError" | "RuntimeError" | "Exception") {
                super::expr::register_error_class(cls.clone());
            } else {
                super::expr::register_class_parent(cls.clone(), type_name(p.0.as_str()));
            }
        }
        for m in &lc.methods {
            if m.kind == AccessorKind::Method && super::expr::body_throws(&m.body) {
                super::expr::register_throws(format!("{cls}.{}", camel(m.name.as_str())));
            }
        }
        if lc.is_module {
            for n in class_accessor_props(&lc.methods).keys() {
                super::expr::register_object_prop(format!("{cls}.{n}"));
            }
        }
    }
}

/// Emit a `LibraryClass` as a standalone Swift file under
/// `Sources/App/app/models/<Name>.swift`.
pub fn emit_class_file(lc: &LibraryClass) -> EmittedFile {
    let class_name = type_name(lc.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("Sources/App/app/models/{class_name}.swift")),
        content: emit_library_class(lc),
    }
}

pub fn emit_library_class(lc: &LibraryClass) -> String {
    let class_name = type_name(lc.name.0.as_str());

    // A Ruby module with class-level state/methods (`module ActiveRecord`
    // with `class << self; attr_accessor :adapter`) → a caseless enum of
    // statics. A set-once reference accessor (the adapter slot) becomes
    // an implicitly-unwrapped `static var` — Swift's lateinit.
    if lc.is_module {
        super::expr::set_instance_prop_types(std::collections::HashMap::new());
        super::expr::set_current_class(&class_name);
        super::expr::set_error_class(false);
        let accessor_props = class_accessor_props(&lc.methods);
        let mut out = format!("enum {class_name} {{\n");
        for (n, ty) in &accessor_props {
            match try_default_for(ty) {
                Some(d) => {
                    out.push_str(&format!("    static var {n}: {} = {d}\n", swift_ty(ty)));
                }
                None => {
                    out.push_str(&format!("    static var {n}: {}!\n", swift_ty(ty)));
                }
            }
        }
        if !accessor_props.is_empty() {
            out.push('\n');
        }
        for m in &lc.methods {
            if m.kind == AccessorKind::Method {
                out.push_str(&indent_method(&emit_method(m, true)));
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
    //    typed from their assign sites when the lowerer stamped one,
    //    else `Any?`.
    let mut body_ivars: BTreeMap<String, IvarInfo> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }

    // 3. Pure readers (`def errors; @errors; end`) collapse into the
    //    property — Swift forbids a same-name property + method (legal
    //    in Kotlin). Their declared returns are the best typing signal
    //    for the backing ivar.
    let mut pure_readers: BTreeMap<String, Ty> = BTreeMap::new();
    for m in &lc.methods {
        if is_pure_reader(m) {
            if let Some(ret) = m_sig_ret(m) {
                if !matches!(ret, Ty::Nil) {
                    pure_readers.insert(camel(m.name.as_str()), ret);
                }
            }
        }
    }

    // Install the property-type map (accessors + typed body ivars +
    // collapsed readers) so the expression walker can coerce
    // untyped-map → typed-property assigns and resolve self-receiver
    // property-vs-method reads.
    let mut all_props: std::collections::HashMap<String, Ty> =
        prop_types.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, info) in &body_ivars {
        let ty = pure_readers
            .get(n)
            .cloned()
            .or_else(|| info.ty.clone())
            .unwrap_or(Ty::Untyped);
        all_props.entry(n.clone()).or_insert(ty);
    }
    super::expr::set_instance_prop_types(all_props);
    super::expr::set_current_class(&class_name);

    let mut out = String::new();

    // Class header + parent. A Ruby error class (`< StandardError`)
    // conforms to Swift's `Error` protocol instead of subclassing — the
    // shape the Phase 3 `throws` pass throws and catches. `Error` has no
    // initializer, so the message contract is a synthesized stored
    // property that `super(msg)` in the init assigns
    // (`expr::set_error_class`).
    let is_error_class = matches!(
        &lc.parent,
        Some(p) if matches!(
            p.0.as_str().rsplit("::").next().unwrap_or(""),
            "StandardError" | "RuntimeError" | "Exception"
        )
    );
    super::expr::set_error_class(is_error_class);
    let header = match &lc.parent {
        Some(_) if is_error_class => format!("class {class_name}: Error"),
        Some(p) => format!("class {class_name}: {}", type_name(p.0.as_str())),
        None => format!("class {class_name}"),
    };
    out.push_str(&header);
    out.push_str(" {\n");
    if is_error_class {
        out.push_str("    var message: String = \"\"\n");
    }

    // Properties. A slot an ANCESTOR already declares is skipped —
    // Swift can't override stored properties (the subclass writes the
    // inherited var). A non-defaultable type (a class reference) emits
    // with no initializer — Swift's definite-initialization accepts
    // that when every `init` assigns it (the ctor-param-backed accessor
    // shape).
    for (n, ty) in &prop_types {
        if super::expr::ancestor_has_prop(&class_name, n) {
            continue;
        }
        match try_default_for(ty) {
            Some(d) => {
                out.push_str(&format!("    var {n}: {} = {d}\n", swift_ty(ty)));
            }
            None => {
                out.push_str(&format!("    var {n}: {}\n", swift_ty(ty)));
            }
        }
    }
    for (n, info) in &body_ivars {
        if prop_types.contains_key(n) || super::expr::ancestor_has_prop(&class_name, n) {
            continue;
        }
        // A collapsed pure reader's declared return is the strongest
        // typing signal; assign-site stamps are the fallback.
        let best = pure_readers.get(n).cloned().or_else(|| info.ty.clone());
        match (&best, info.saw_nil) {
            (Some(t), false) => {
                out.push_str(&format!("    var {n}: {} = {}\n", swift_ty(t), default_for(t)));
            }
            (Some(t), true) => {
                let mut st = swift_ty(t);
                if !st.ends_with('?') {
                    st.push('?');
                }
                out.push_str(&format!("    var {n}: {st} = nil\n"));
            }
            (None, _) => {
                out.push_str(&format!("    var {n}: Any? = nil\n"));
            }
        }
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() {
        out.push('\n');
    }

    let ctx = ClassCtx { name: class_name.clone(), has_parent: lc.parent.is_some() && !is_error_class };

    // Ruby `[]` / `[]=` pairs merge into ONE Swift `subscript`
    // declaration (they can't be split like Kotlin's operator funs).
    let sub_get = lc.methods.iter().find(|m| {
        m.receiver == MethodReceiver::Instance && m.kind == AccessorKind::Method && m.name.as_str() == "[]"
    });
    let sub_set = lc.methods.iter().find(|m| {
        m.receiver == MethodReceiver::Instance && m.kind == AccessorKind::Method && m.name.as_str() == "[]="
    });
    if sub_get.is_some() || sub_set.is_some() {
        let needs_override = super::expr::ancestor_has(&class_name, "__subscript__", false);
        out.push_str(&indent_method(&emit_subscript(sub_get, sub_set, needs_override)));
        out.push('\n');
    }

    // Instance methods (skip accessors, pure readers, and the merged
    // subscript pair).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance
            && m.kind == AccessorKind::Method
            && !matches!(m.name.as_str(), "[]" | "[]=")
            && !is_pure_reader(m)
        {
            out.push_str(&indent_method(&emit_method_in(m, false, &ctx)));
            out.push('\n');
        }
    }

    // Class methods → `class func` members (overridable — Swift statics
    // are inherited, and the per-model overrides of tableName/
    // _adapter_* need dynamic `Self` dispatch).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Class && m.kind == AccessorKind::Method {
            out.push_str(&indent_method(&emit_method_in(m, true, &ctx)));
            out.push('\n');
        }
    }

    // `#{name}` in NotImplementedError messages reads Ruby's
    // `Class#name` — synthesize it when referenced and not defined
    // (Ruby-qualified name, per class).
    let defines_name = lc.methods.iter().any(|m| m.name.as_str() == "name");
    let references_name = lc.methods.iter().any(|m| references_class_name(&m.body));
    if references_name && !defines_name {
        let override_kw = if super::expr::ancestor_has(&class_name, "name", true) {
            "override "
        } else {
            ""
        };
        out.push_str(&format!(
            "    {override_kw}class func name() -> String {{\n        return \"{}\"\n    }}\n",
            lc.name.0.as_str()
        ));
    }

    out.push_str("}\n");
    out
}

/// Context for method emission inside a `class` (vs a module enum).
struct ClassCtx {
    name: String,
    has_parent: bool,
}

/// A bare zero-arg `name` send (Ruby's `Class#name`) anywhere in a body.
fn references_class_name(e: &Expr) -> bool {
    if let ExprNode::Send { recv, method, args, .. } = &*e.node {
        let self_recv = match recv {
            None => true,
            Some(r) => matches!(&*r.node, ExprNode::SelfRef),
        };
        if method.as_str() == "name" && args.is_empty() && self_recv {
            return true;
        }
    }
    expr_children(e).into_iter().any(references_class_name)
}

/// Merged Swift `subscript` from the Ruby `[]` / `[]=` pair.
fn emit_subscript(
    getter: Option<&MethodDef>,
    setter: Option<&MethodDef>,
    needs_override: bool,
) -> String {
    let model = getter.or(setter).expect("subscript needs at least one of []/[]=");
    let (param_name, param_ty) = match m_sig_params(model).and_then(|p| p.first()) {
        Some(p) => (camel(p.name.as_str()), swift_ty(&p.ty)),
        None => ("name".to_string(), "String".to_string()),
    };
    let elem_ty = match getter.and_then(m_sig_ret) {
        Some(t) if !matches!(t, Ty::Nil) => swift_ty(&t),
        _ => setter
            .and_then(m_sig_params)
            .and_then(|p| p.get(1).map(|p| swift_ty(&p.ty)))
            .unwrap_or_else(|| "Any?".to_string()),
    };
    let mut out = format!(
        "{}subscript({param_name}: {param_ty}) -> {elem_ty} {{\n",
        if needs_override { "override " } else { "" }
    );
    if let Some(g) = getter {
        begin_method(&g.body, true);
        out.push_str(&format!("    get {{\n{}\n    }}\n", indent4(&indent4(&emit_body(&g.body, true, None)))));
    }
    if let Some(s) = setter {
        begin_method(&s.body, false);
        let body = emit_body(&s.body, false, None);
        // The Ruby setter's value param reads from Swift's `newValue`.
        let alias = s
            .params
            .get(1)
            .map(|p| format!("    let {} = newValue\n", camel(p.name.as_str())))
            .unwrap_or_default();
        out.push_str(&format!(
            "    set {{\n{alias}{}\n    }}\n",
            indent4(&indent4(&body))
        ));
    }
    out.push_str("}\n");
    out
}

fn m_sig_params(m: &MethodDef) -> Option<&Vec<crate::ty::Param>> {
    match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    }
}

fn m_sig_ret(m: &MethodDef) -> Option<Ty> {
    match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    }
}

fn indent_method(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The ivar a pure-reader body returns (`@errors` for
/// `def errors; @errors; end`), camelCased.
fn pure_reader_target(body: &Expr) -> Option<String> {
    match &*body.node {
        ExprNode::Ivar { name } => Some(camel(name.as_str())),
        ExprNode::Return { value } => pure_reader_target(value),
        ExprNode::Seq { exprs } if exprs.len() == 1 => pure_reader_target(&exprs[0]),
        _ => None,
    }
}

/// Is this method a pure reader that collapses into its backing ivar?
fn is_pure_reader(m: &MethodDef) -> bool {
    m.receiver == MethodReceiver::Instance
        && m.kind == AccessorKind::Method
        && m.params.is_empty()
        && pure_reader_target(&m.body).as_deref() == Some(camel(m.name.as_str()).as_str())
}

/// All stored-property names a class declares: instance accessors, body
/// ivars, collapsed pure readers.
fn instance_prop_names(lc: &LibraryClass) -> std::collections::HashSet<String> {
    let mut props: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                props.insert(camel(m.name.as_str()));
            }
            AccessorKind::AttributeWriter => {
                props.insert(camel(m.name.as_str().trim_end_matches('=')));
            }
            AccessorKind::Method => {
                if is_pure_reader(m) {
                    props.insert(camel(m.name.as_str()));
                }
            }
        }
    }
    let mut ivars: BTreeMap<String, IvarInfo> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut ivars);
    }
    props.extend(ivars.keys().cloned());
    props
}

/// Module-context method emit (enums use `static`, no overrides).
fn emit_method(m: &MethodDef, is_static: bool) -> String {
    emit_method_impl(m, is_static, None)
}

/// Class-context method emit (`class func` statics, `override` marking,
/// `required init` + `super.init` delegation).
fn emit_method_in(m: &MethodDef, is_static: bool, ctx: &ClassCtx) -> String {
    emit_method_impl(m, is_static, Some(ctx))
}

fn emit_method_impl(m: &MethodDef, is_static: bool, ctx: Option<&ClassCtx>) -> String {
    // Ruby `[]` / `[]=` are emitted as plain `get`/`set` funcs for now;
    // merging the pair into a Swift `subscript` declaration is a Phase 3
    // concern (the runtime's ActiveRecord Base is the only definer).
    let (name, force_unit) = match m.name.as_str() {
        "[]" => ("get".to_string(), false),
        "[]=" => ("set".to_string(), true),
        _ => (camel(m.name.as_str()), false),
    };

    // Params — always typed (Swift requirement), underscore-labeled so
    // the positional lowered call sites work.
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    };
    let params: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pn = camel(p.name.as_str());
            let ty = sig_params
                .and_then(|sp| sp.get(i))
                .map(|sp| swift_ty(&sp.ty))
                .unwrap_or_else(|| "Any?".to_string());
            match &p.default {
                Some(d) => format!("_ {pn}: {ty} = {}", emit_expr(d)),
                None => format!("_ {pn}: {ty}"),
            }
        })
        .collect();

    // Ruby `initialize` → a real Swift `init` (no func keyword, no
    // return clause; `self.`-qualified property assigns let ctor params
    // shadow the properties).
    let is_init = !is_static && m.name.as_str() == "initialize";

    // Return type.
    let ret_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let returns_value =
        !is_init && !force_unit && matches!(&ret_ty, Some(t) if !matches!(t, Ty::Nil));
    let ret_clause = if force_unit || is_init {
        String::new()
    } else {
        match &ret_ty {
            Some(t) if !matches!(t, Ty::Nil) => format!(" -> {}", swift_ty(t)),
            _ => String::new(),
        }
    };

    begin_method(&m.body, returns_value);
    super::expr::set_init_super(is_init && ctx.map_or(false, |c| c.has_parent));
    let body = emit_body(&m.body, returns_value, ret_ty.as_ref());
    super::expr::set_init_super(false);

    if is_init {
        // `required` so `Self(attrs)` (the implicit-self `new`) is legal
        // in class methods; it also covers the subclass-init override
        // contract (required replaces `override` for inits).
        let required_kw = if ctx.is_some() { "required " } else { "" };
        return format!(
            "{required_kw}init({}) {{\n{}\n}}\n",
            params.join(", "),
            indent4(&body)
        );
    }
    // The throws split (plan delta 1): a method whose body throws an
    // Error-conforming class carries `throws`.
    let throws_kw = if super::expr::body_throws(&m.body) { " throws" } else { "" };
    let static_kw = if is_static {
        // In classes, statics emit `class func` so per-model overrides
        // (tableName, _adapter_*) dispatch dynamically via `Self`.
        if ctx.is_some() { "class " } else { "static " }
    } else {
        ""
    };
    let override_kw = match ctx {
        Some(c) if is_static => {
            // Covariant CLASS returns override legally; covariant
            // CONTAINER returns can only shadow (an overload the typed
            // assignment context resolves) — `override` there is a
            // compile error.
            let my_ret = match &ret_ty {
                Some(t) if !matches!(t, Ty::Nil) => swift_ty(t),
                _ => String::new(),
            };
            match super::expr::ancestor_static_ret(&c.name, &name) {
                Some(anc) if anc == my_ret || super::expr::is_same_or_descendant(&my_ret, &anc) => {
                    "override "
                }
                _ => "",
            }
        }
        Some(c) if super::expr::ancestor_has(&c.name, &name, false) => "override ",
        _ => "",
    };
    format!(
        "{override_kw}{static_kw}func {name}({}){throws_kw}{ret_clause} {{\n{}\n}}\n",
        params.join(", "),
        indent4(&body)
    )
}

fn indent4(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit a method body, adding an explicit `return` to the final statement
/// when the method returns a value (Ruby implicit return → Swift). An
/// EMPTY value-returning body (the load-bearing-empty `_adapter_*` /
/// association-stub pattern) synthesizes a default return so the file
/// compiles.
fn emit_body(body: &Expr, returns_value: bool, ret_ty: Option<&Ty>) -> String {
    if !returns_value {
        return emit_expr(body);
    }
    if is_empty_body(body) {
        return default_return(ret_ty);
    }
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => super::expr::emit_stmts(exprs, true),
        _ => wrap_return(body),
    }
}

fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: crate::expr::Literal::Nil })
}

/// The synthesized statement for an empty value-returning body.
fn default_return(ret_ty: Option<&Ty>) -> String {
    match ret_ty {
        Some(Ty::Int) => "return 0".to_string(),
        Some(Ty::Float) => "return 0.0".to_string(),
        Some(Ty::Bool) => "return false".to_string(),
        Some(Ty::Str) | Some(Ty::Sym) => "return \"\"".to_string(),
        Some(Ty::Array { .. }) => "return []".to_string(),
        Some(Ty::Hash { .. }) => "return [:]".to_string(),
        Some(Ty::Union { variants })
            if variants.iter().any(|v| matches!(v, Ty::Nil)) =>
        {
            "return nil".to_string()
        }
        _ => "fatalError(\"unimplemented\")".to_string(),
    }
}

/// Body-ivar inventory: camelCased name → inferred declaration. The type
/// comes from assign sites — the first concrete `Ty` stamped on an
/// assigned value (the lowerer's hash-field Ty stamping reaches ivar
/// assigns) wins; an ivar that is ever assigned a `nil` literal becomes
/// optional. No signal at all degrades to `Any?` — the same Any?-soup
/// Kotlin started with, escaped the same way.
#[derive(Default, Clone)]
struct IvarInfo {
    ty: Option<Ty>,
    saw_nil: bool,
}

fn collect_ivars(e: &Expr, out: &mut BTreeMap<String, IvarInfo>) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            out.entry(camel(name.as_str())).or_default();
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let info = out.entry(camel(name.as_str())).or_default();
            if matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil }) {
                info.saw_nil = true;
            } else if info.ty.is_none() {
                if let Some(t) = value.ty.as_ref() {
                    if !matches!(t, Ty::Nil | Ty::Untyped | Ty::Var { .. }) {
                        info.ty = Some(t.clone());
                    }
                }
            }
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

/// Default initializer for a property type (emitted stored properties
/// are initialized so subclasses keep the inherited memberwise-free
/// default `init`). `None` for non-defaultable types — a bare class
/// reference can only be init-assigned.
fn try_default_for(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Array { .. } => "[]".to_string(),
        Ty::Hash { .. } => "[:]".to_string(),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "nil".to_string()
        }
        Ty::Untyped | Ty::Var { .. } => "nil".to_string(),
        _ => return None,
    })
}

fn default_for(ty: &Ty) -> String {
    try_default_for(ty).unwrap_or_else(|| "nil".to_string())
}
