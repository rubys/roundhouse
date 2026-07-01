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
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::ty::{Param, ParamKind, Ty};

/// Emit an `.rbs` sidecar for a single `LibraryClass`. The output
/// path mirrors `rb_path` under a top-level `sig/` tree with the
/// extension swapped to `.rbs`.
pub(super) fn emit_library_class_rbs(lc: &LibraryClass, rb_path: &Path) -> EmittedFile {
    let path = sig_path_for(rb_path);
    let content = render_class(lc);
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

fn render_class(lc: &LibraryClass) -> String {
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

    for m in &lc.methods {
        let line = render_method(m);
        writeln!(s, "{body_pad}{line}").unwrap();
    }

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }

    s
}

fn render_method(m: &MethodDef) -> String {
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
        return render_def(m);
    }
    match m.kind {
        AccessorKind::AttributeReader => render_attr_reader(m),
        AccessorKind::AttributeWriter => render_attr_writer(m),
        AccessorKind::Method => render_def(m),
    }
}

fn render_attr_reader(m: &MethodDef) -> String {
    let ty = match &m.signature {
        Some(Ty::Fn { ret, .. }) => ty_to_rbs(ret),
        _ => "untyped".to_string(),
    };
    format!("attr_reader {}: {}", m.name.as_str(), ty)
}

fn render_attr_writer(m: &MethodDef) -> String {
    let ty = match &m.signature {
        Some(Ty::Fn { params, .. }) if !params.is_empty() => ty_to_rbs(&params[0].ty),
        _ => "untyped".to_string(),
    };
    let bare = m.name.as_str().trim_end_matches('=');
    format!("attr_writer {}: {}", bare, ty)
}

fn render_def(m: &MethodDef) -> String {
    let receiver_prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };
    let sig = match &m.signature {
        Some(Ty::Fn { params, block, ret, .. }) => {
            let params_str = render_typed_params(params);
            let block_str = match block.as_deref() {
                Some(b) => format!(" {{ {} }}", render_block_ty(b)),
                None => String::new(),
            };
            let ret_str = ty_to_rbs(ret);
            format!("({}){} -> {}", params_str, block_str, ret_str)
        }
        _ => render_untyped_fallback(m),
    };
    format!("def {receiver_prefix}{}: {sig}", m.name.as_str())
}

fn render_untyped_fallback(m: &MethodDef) -> String {
    let parts: Vec<String> = m
        .params
        .iter()
        .map(|p| {
            let prefix = if p.default.is_some() { "?" } else { "" };
            format!("{prefix}untyped {}", p.name.as_str())
        })
        .collect();
    format!("({}) -> untyped", parts.join(", "))
}

fn render_typed_params(params: &[Param]) -> String {
    // Group: required pos, optional pos, rest, required kw, optional kw,
    // kw rest, block (block handled outside). RBS requires a specific
    // order; the IR already carries them in that order from the lowerers.
    let mut parts = Vec::new();
    for p in params {
        let name = p.name.as_str();
        let ty = ty_to_rbs(&p.ty);
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

fn render_block_ty(b: &Ty) -> String {
    match b {
        Ty::Fn { params, ret, .. } => {
            let p = render_typed_params(params);
            format!("({p}) -> {}", ty_to_rbs(ret))
        }
        // Block slot containing a non-Fn type is unusual; fall back to
        // an untyped block contract.
        _ => "(*untyped) -> untyped".to_string(),
    }
}

/// Render a `Ty` as an RBS type expression.
pub fn ty_to_rbs(ty: &Ty) -> String {
    match ty {
        Ty::Int => "Integer".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "String".into(),
        Ty::Sym => "Symbol".into(),
        // Ruby has a native `Time`; datetime columns hydrate to it via
        // apply_datetime_lowering.
        Ty::Time => "Time".into(),
        Ty::Nil => "nil".into(),
        Ty::Array { elem } => format!("Array[{}]", ty_to_rbs(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", ty_to_rbs(key), ty_to_rbs(value)),
        Ty::Tuple { elems } => {
            let inner: Vec<String> = elems.iter().map(ty_to_rbs).collect();
            format!("[{}]", inner.join(", "))
        }
        Ty::Record { row } => {
            let inner: Vec<String> = row
                .fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k.as_str(), ty_to_rbs(v)))
                .collect();
            format!("{{ {} }}", inner.join(", "))
        }
        Ty::Union { variants } => render_union(variants),
        Ty::Class { id, args } => {
            let raw = id.0.as_str();
            if args.is_empty() {
                raw.to_string()
            } else {
                let a: Vec<String> = args.iter().map(ty_to_rbs).collect();
                format!("{raw}[{}]", a.join(", "))
            }
        }
        Ty::Fn { params, ret, .. } => {
            // Procs in value position render as `^(Params) -> Ret`.
            let p = render_typed_params(params);
            format!("^({p}) -> {}", ty_to_rbs(ret))
        }
        Ty::Var { .. } => "untyped".into(),
        Ty::Untyped => "untyped".into(),
        Ty::Bottom => "bot".into(),
    }
}

fn render_union(variants: &[Ty]) -> String {
    // `T | nil` collapses to `T?` (RBS idiomatic optional form).
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    // Dedup structurally-equal members, preserving first-seen order. A
    // `case`/branch return like `@id | @title | @body` typed
    // `Integer | String | String` should render `(Integer | String)`,
    // not repeat the `String`.
    let mut non_nil: Vec<&Ty> = Vec::new();
    for v in variants.iter().filter(|v| !matches!(v, Ty::Nil)) {
        if !non_nil.contains(&v) {
            non_nil.push(v);
        }
    }
    if has_nil && non_nil.len() == 1 {
        return format!("{}?", ty_to_rbs(non_nil[0]));
    }
    if non_nil.is_empty() {
        // All-Nil union; degenerate but represent it.
        return "nil".into();
    }
    let rendered: Vec<String> = non_nil.iter().map(|t| ty_to_rbs(t)).collect();
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
            let file_stem = crate::naming::snake_case(lc.name.0.as_str());
            let rb_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            emit_library_class_rbs(lc, &rb_path)
        })
        .collect()
}
