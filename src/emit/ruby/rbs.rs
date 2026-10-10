//! RBS sidecar emission for library-shape Ruby output.
//!
//! Produces `.rbs` files under a top-level `sig/` tree mirroring the
//! `.rb` layout (`app/models/article.rb` → `sig/app/models/article.rbs`).
//! Spinel #571 walks `--rbs DIR` recursively and accepts either
//! layout, but Steep / TypeProf auto-discover `sig/` by convention —
//! so this layout costs zero extra config for either consumer.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::super::EmittedFile;
use crate::dialect::{
    AccessorKind, LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver, MethodVisibility,
};
use crate::expr::{Expr, ExprNode, Literal, RESOLVED_DATA_FACTORY};
use crate::ty::{Param, ParamKind, Ty};

/// Emit an `.rbs` sidecar for a single `LibraryClass`. The output
/// path mirrors `rb_path` under a top-level `sig/` tree with the
/// extension swapped to `.rbs`.
pub(super) fn emit_library_class_rbs(lc: &LibraryClass, rb_path: &Path) -> EmittedFile {
    emit_library_class_rbs_with_factories(lc, rb_path, &[])
}

/// Emit the `sig/` sidecar for an owner and its Data factory constants. `classes`
/// supplies lifted custom methods, matched to declarations by their source spans.
pub(super) fn emit_library_class_rbs_with_factories(
    lc: &LibraryClass,
    rb_path: &Path,
    classes: &[LibraryClass],
) -> EmittedFile {
    let path = sig_path_for(rb_path);
    let content = render_class(lc, classes);
    EmittedFile { path, content }
}

/// Compute the `sig/`-rooted destination path for a `.rb` source path.
/// `app/models/article.rb` → `sig/app/models/article.rbs`. Idempotent
/// if `rb_path` already starts with `sig/` (defensive — currently no
/// caller passes such a path).
fn sig_path_for(rb_path: &Path) -> PathBuf {
    let with_rbs_ext = rb_path.with_extension("rbs");
    if with_rbs_ext.starts_with("sig") {
        with_rbs_ext
    } else {
        PathBuf::from("sig").join(with_rbs_ext)
    }
}

/// Render a library class or module with enclosing namespaces, includes, method
/// signatures, and admitted Data factories nested under their constant owner.
fn render_class(lc: &LibraryClass, classes: &[LibraryClass]) -> String {
    let mut s = String::new();
    let name = lc.name.0.as_str();
    let segments: Vec<&str> = name.split("::").collect();
    let depth = segments.len();

    if lc.is_module {
        for (i, seg) in segments.iter().enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
    } else {
        for (i, seg) in segments.iter().take(depth - 1).enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
        let last = segments[depth - 1];
        let pad = "  ".repeat(depth - 1);
        match lc.parent.as_ref() {
            Some(p) => writeln!(s, "{pad}class {last} < {}", p.0.as_str()).unwrap(),
            None => writeln!(s, "{pad}class {last}").unwrap(),
        }
    }

    let body_pad = "  ".repeat(depth);

    for inc in &lc.includes {
        writeln!(s, "{body_pad}include {}", inc.0.as_str()).unwrap();
    }
    if !lc.includes.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    for (name, value) in &lc.constants {
        let factory = classes.iter().find(|class| {
            matches!(class.origin, Some(LibraryClassOrigin::DataFactory { declaration_span })
                if declaration_span == value.span)
        });
        render_data_factory(&mut s, lc, name.as_str(), value, &body_pad, factory);
    }

    for m in &lc.methods {
        let line = render_method(m, &segments);
        writeln!(s, "{body_pad}{line}").unwrap();
    }

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }

    s
}

/// Render an admitted factory as a nested Data subclass; leave unrelated or
/// unresolved constants alone. Keep generated constructor/readers untyped and
/// omit a generated reader when the custom block supplies that method.
fn render_data_factory(s: &mut String, owner: &LibraryClass, name: &str, value: &Expr, pad: &str, factory: Option<&LibraryClass>) {
    if value.decisions & RESOLVED_DATA_FACTORY == 0 {
        return;
    }
    let Some(Ty::Class { id, args: type_args }) = &value.ty else { return };
    if !type_args.is_empty() || id.0.as_str() != format!("{}::{name}", owner.name.0.as_str()) {
        return;
    }
    let ExprNode::Send { args, .. } = &*value.node else { return };
    let Some(members) = args.iter().map(|arg| match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
        _ => None,
    }).collect::<Option<Vec<_>>>() else { return };
    writeln!(s, "{pad}class {name} < ::Data").unwrap();
    writeln!(s, "{pad}  def self.new: (*untyped, **untyped) -> instance").unwrap();
    for member in members {
        if factory.is_some_and(|class| {
            class.methods.iter().any(|method| {
                method.receiver == MethodReceiver::Instance && method.name.as_str() == member
            })
        }) {
            continue;
        }
        writeln!(s, "{pad}  def `{member}`: () -> untyped").unwrap();
    }
    if let Some(factory) = factory {
        let enclosing: Vec<_> = factory.name.0.as_str().split("::").collect();
        for method in &factory.methods {
            // RBS has no protected visibility; private is the conservative
            // representation for both non-public Ruby instance visibilities.
            let visibility = match method.visibility {
                MethodVisibility::Public => "",
                MethodVisibility::Protected | MethodVisibility::Private => "private ",
            };
            writeln!(
                s,
                "{pad}  {visibility}{}",
                render_method(method, &enclosing)
            )
            .unwrap();
        }
    }
    writeln!(s, "{pad}end").unwrap();
}

fn render_method(m: &MethodDef, enclosing: &[&str]) -> String {
    // Class-receiver methods can't be attr_reader / attr_writer at the
    // RBS surface — the `attr_*` shorthand only describes instance
    // attributes, and an `attr_reader name?: bool` form lacks any way
    // to express singleton-scope. `def self.abstract?; true; end` flows
    // through the lowerer with AccessorKind::AttributeReader (predicate
    // shape, no body argument), but emitting it as `attr_reader
    // abstract?: bool` then trips spinel's RBS extractor into adding an
    // ivar named `@abstract?` — invalid C identifier. Fall through to
    // `def self.name` rendering for any class-receiver method.
    if matches!(m.receiver, MethodReceiver::Class) {
        return render_def(m, enclosing);
    }
    match m.kind {
        AccessorKind::AttributeReader => render_attr_reader(m, enclosing),
        AccessorKind::AttributeWriter => render_attr_writer(m, enclosing),
        AccessorKind::Method => render_def(m, enclosing),
    }
}

fn render_attr_reader(m: &MethodDef, enclosing: &[&str]) -> String {
    let ty = match &m.signature {
        Some(Ty::Fn { ret, .. }) => ty_to_rbs_in(ret, enclosing),
        _ => "untyped".to_string(),
    };
    format!("attr_reader {}: {}", m.name.as_str(), ty)
}

fn render_attr_writer(m: &MethodDef, enclosing: &[&str]) -> String {
    let ty = match &m.signature {
        Some(Ty::Fn { params, .. }) if !params.is_empty() => {
            ty_to_rbs_in(&params[0].ty, enclosing)
        }
        _ => "untyped".to_string(),
    };
    let bare = m.name.as_str().trim_end_matches('=');
    format!("attr_writer {}: {}", bare, ty)
}

fn render_def(m: &MethodDef, enclosing: &[&str]) -> String {
    let receiver_prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };
    // A full forwarder has no named rest binding. Neither its keywords
    // nor its block can be inferred from the synthetic signature slot.
    let sig = match &m.signature {
        Some(Ty::Fn { params, block, ret, .. }) if !m.params.iter().any(|p| p.forwarding) => {
            let params_str = render_typed_params(params, enclosing);
            let block_str = match block.as_deref() {
                Some(b) => format!(" {{ {} }}", render_block_ty(b, enclosing)),
                None => String::new(),
            };
            let ret_str = ty_to_rbs_in(ret, enclosing);
            format!("({}){} -> {}", params_str, block_str, ret_str)
        }
        _ => render_untyped_fallback(m),
    };
    format!("def {receiver_prefix}{}: {sig}", m.name.as_str())
}

fn render_untyped_fallback(m: &MethodDef) -> String {
    // Kind-aware like render_typed_params — a keyword param rendered
    // positionally makes the sig disagree with the def (spinel then
    // mis-binds the call and type-checks kwarg defaults in the
    // caller's context).
    let parts: Vec<String> = m
        .params
        .iter()
        .map(|p| {
            let name = p.name.as_str();
            let optional = if p.default.is_some() { "?" } else { "" };
            if p.forwarding {
                "*untyped, **untyped".to_string()
            } else if p.keyword && p.rest {
                format!("**untyped {name}")
            } else if p.keyword {
                format!("{optional}{name}: untyped")
            } else if p.rest {
                format!("*untyped {name}")
            } else {
                format!("{optional}untyped {name}")
            }
        })
        .collect();
    let block = if m.params.iter().any(|p| p.forwarding) {
        " ?{ (*untyped, **untyped) -> untyped }"
    } else { "" };
    format!("({}){block} -> untyped", parts.join(", "))
}

fn render_typed_params(params: &[Param], enclosing: &[&str]) -> String {
    // Group: required pos, optional pos, rest, required kw, optional kw,
    // kw rest, block (block handled outside). RBS requires a specific
    // order; the IR already carries them in that order from the lowerers.
    let mut parts = Vec::new();
    for p in params {
        let name = p.name.as_str();
        // IR keyword-rest params name the collected Hash, while RBS
        // **T names the type of each keyword value (see rbs ingestion).
        let param_ty = match (&p.kind, &p.ty) {
            (ParamKind::KeywordRest, Ty::Hash { value, .. }) => &**value,
            _ => &p.ty,
        };
        let mut ty = ty_to_rbs_in(param_ty, enclosing);
        // A parameter whose every observed call site passed nil is not a
        // `nil`-typed parameter: seeded as such, spinel refuses the method
        // ("param has unsupported type nil"). Widen to untyped.
        if ty == "nil" {
            ty = "untyped".to_string();
        }
        let part = match p.kind {
            ParamKind::Required => format!("{ty} {name}"),
            ParamKind::Optional => format!("?{ty} {name}"),
            ParamKind::Rest => format!("*{ty} {name}"),
            ParamKind::Keyword { required: true } => format!("{name}: {ty}"),
            ParamKind::Keyword { required: false } => format!("?{name}: {ty}"),
            ParamKind::KeywordRest => format!("**{ty} {name}"),
            ParamKind::Block => continue,
        };
        parts.push(part);
    }
    parts.join(", ")
}

fn render_block_ty(b: &Ty, enclosing: &[&str]) -> String {
    match b {
        Ty::Fn { params, ret, .. } => {
            let p = render_typed_params(params, enclosing);
            format!("({p}) -> {}", ty_to_rbs_in(ret, enclosing))
        }
        // Block slot containing a non-Fn type is unusual; fall back to
        // an untyped block contract.
        _ => "(*untyped) -> untyped".to_string(),
    }
}

/// Render a `Ty` as an RBS type expression, at the top level.
pub fn ty_to_rbs(ty: &Ty) -> String {
    ty_to_rbs_in(ty, &[])
}

/// Render a `Ty` as an RBS type expression inside the declaration
/// `enclosing` names (its `module`/`class` segments, outermost first).
///
/// A type name in RBS resolves LEXICALLY, as a constant does in Ruby:
/// inside `module Views; module Search`, `Search` is `Views::Search`.
/// Every `Ty::Class` id here is the top-level name, so one whose first
/// segment is also a NESTED enclosing segment is written rooted
/// (`::Search`), which names the same class from anywhere. lobsters'
/// search view is exactly that shape — `Views::Search.index_into(io,
/// Search search, …)` — and spinel, once it read the name lexically,
/// typed `search` as the view module and refused
/// `search.total_results > -1`. The outermost segment is not a
/// collision: inside `class Domain`, `Domain` resolves to the top-level
/// `Domain` it names, so a model's references to itself stay bare and
/// only the shadowed names change.
fn ty_to_rbs_in(ty: &Ty, enclosing: &[&str]) -> String {
    let rbs = |t: &Ty| ty_to_rbs_in(t, enclosing);
    match ty {
        Ty::Int => "Integer".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "String".into(),
        Ty::Sym => "Symbol".into(),
        Ty::Date => "Date".into(),
        // Ruby has a native `Time`; datetime columns hydrate to it via
        // apply_datetime_lowering.
        Ty::Time => "Time".into(),
        Ty::Nil => "nil".into(),
        // RBS can SPELL this one — `instance` is its own syntax, and
        // it is where roundhouse reads it from. It is still a
        // diagnostic rather than a round-trip: `dispatch` substitutes
        // a self type with the receiving class before the type is
        // stored or joined (see `Ty::SelfInstance`), so one arriving
        // here is a signature that was read and never dispatched.
        // Printing `instance` back out would make that defect
        // invisible precisely because the output stays valid.
        Ty::SelfInstance => {
            crate::emit::diagnostics::unsupported_self_instance_ty("rbs")
        }
        // Analysis-time relation type. On the STRICT targets reaching
        // here is a coverage gap — a chain that escaped the arel fold
        // with no runtime to fall back on — and their renderers report
        // it. The ruby family is the exception this arm exists for:
        // `runtime/ruby/active_record/relation.rb` IS a real class, the
        // emitted code names it (`ActiveRecord::Relation.new(Room)` is
        // what a `has_many :through` reader returns), and the
        // `lower_residue` diagnostic already says so in as many words —
        // "executes on the runtime Relation in ruby-family targets,
        // unsupported at strict-target emit". Naming it here is not a
        // degrade to `Array[T]`; it is the type the method actually
        // has. The class is non-generic in v1 (its element type is
        // `untyped`), so the `of` is dropped rather than rendered.
        Ty::Relation { .. } => "ActiveRecord::Relation".into(),
        Ty::Array { elem } => format!("Array[{}]", rbs(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", rbs(key), rbs(value)),
        Ty::Tuple { elems } => {
            let inner: Vec<String> = elems.iter().map(rbs).collect();
            format!("[{}]", inner.join(", "))
        }
        Ty::Record { row } => {
            let inner: Vec<String> = row
                .fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k.as_str(), rbs(v)))
                .collect();
            format!("{{ {} }}", inner.join(", "))
        }
        Ty::Union { variants } => render_union(variants, enclosing),
        // The class object itself (`class_object_return_ty`), which RBS
        // spells `singleton(C)`; `Class[C]` is not an RBS type.
        Ty::Class { id, args } if id.0.as_str() == "Class" && args.len() == 1 => {
            format!("singleton({})", rbs(&args[0]))
        }
        Ty::Class { id, args } => {
            let raw = id.0.as_str();
            let first = raw.split("::").next().unwrap_or(raw);
            let name = if enclosing.iter().skip(1).any(|seg| *seg == first) {
                format!("::{raw}")
            } else {
                raw.to_string()
            };
            if args.is_empty() {
                name
            } else {
                let a: Vec<String> = args.iter().map(rbs).collect();
                format!("{name}[{}]", a.join(", "))
            }
        }
        Ty::Fn { params, ret, .. } => {
            // Procs in value position render as `^(Params) -> Ret`.
            let p = render_typed_params(params, enclosing);
            format!("^({p}) -> {}", rbs(ret))
        }
        Ty::Var { .. } => "untyped".into(),
        Ty::Untyped => "untyped".into(),
        Ty::Bottom => "bot".into(),
    }
}

fn render_union(variants: &[Ty], enclosing: &[&str]) -> String {
    // `String | untyped` IS `untyped` — the gradual arm subsumes every
    // other one, so rendering the members alongside it advertises a
    // precision the type does not have. Spinel reads `(String |
    // untyped)` as a real union and emits the poly dispatch anyway, so
    // the fake arm costs speed on top of honesty. Collapse first,
    // before the `T?` sugar, or a `T | untyped | nil` renders the
    // equally-meaningless `untyped?`.
    if variants.iter().any(|v| matches!(v, Ty::Untyped | Ty::Var { .. })) {
        return "untyped".into();
    }
    // `T | nil` collapses to `T?` (RBS idiomatic optional form).
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    // Dedup members, preserving first-seen order. A `case`/branch
    // return like `@id | @title | @body` typed `Integer | String |
    // String` should render `(Integer | String)`, not repeat the
    // `String`. By the RENDERED text, not the `Ty`: members RBS cannot
    // tell apart print the same (`Relation { Story } | Relation {
    // Comment }` is `ActiveRecord::Relation` twice — lobsters'
    // `searched_model.none`, #132).
    let mut rendered: Vec<String> = Vec::new();
    for v in variants.iter().filter(|v| !matches!(v, Ty::Nil)) {
        let r = ty_to_rbs_in(v, enclosing);
        if !rendered.contains(&r) {
            rendered.push(r);
        }
    }
    if has_nil && rendered.len() == 1 {
        // A nested union (`(T | nil) | nil`) already rendered `T?`.
        if rendered[0].ends_with('?') {
            return rendered.into_iter().next().unwrap();
        }
        return format!("{}?", rendered[0]);
    }
    if rendered.is_empty() {
        // All-Nil union; degenerate but represent it.
        return "nil".into();
    }
    if has_nil {
        format!("({} | nil)", rendered.join(" | "))
    } else if rendered.len() == 1 {
        rendered.into_iter().next().unwrap()
    } else {
        format!("({})", rendered.join(" | "))
    }
}

/// Convenience: emit `.rbs` sidecars for every `LibraryClass` in
/// `app.library_classes`. Mirrors `library::emit_library_class_decls`.
#[allow(dead_code)]
pub(super) fn emit_library_class_rbs_decls(app: &crate::App) -> Vec<EmittedFile> {
    app.library_classes
        .iter()
        .map(|lc| {
            let file_stem = crate::naming::underscore(lc.name.0.as_str());
            let rb_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            emit_library_class_rbs(lc, &rb_path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;

    #[test]
    fn keyword_rest_renders_values_not_a_nested_hash() {
        let hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) };
        let params = [
            Param { name: Symbol::from("mapping"), ty: hash.clone(), kind: ParamKind::Required },
            Param { name: Symbol::from("opts"), ty: hash, kind: ParamKind::KeywordRest },
        ];
        assert_eq!(render_typed_params(&params, &[]), "Hash[Symbol, String] mapping, **String opts");
    }
}
