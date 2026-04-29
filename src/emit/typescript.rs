//! TypeScript emitter — rebuild in progress.
//!
//! Being rebuilt slice-by-slice against the spinel-blog canonical
//! output shape (see project_emitter_rip_and_replace memory). Each
//! commit lands one slice; the 32 ignored TS tests under tests/ are
//! the re-entry gate.
//!
//! Slice 1 (this revision): package.json + main.ts.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");
const HTTP_STUB_SOURCE: &str = include_str!("../../runtime/typescript/http.ts");
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/typescript/test_support.ts");
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/typescript/view_helpers.ts");
const SERVER_SOURCE: &str = include_str!("../../runtime/typescript/server.ts");

mod controller;
mod expr;
mod fixture;
mod library;
mod main_ts;
mod model;
mod model_from_library;
mod naming;
mod package;
mod route;
mod route_helpers;
mod schema_sql;
mod spec;
mod ty;
mod view;

pub use ty::ts_ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    emit_with_adapter(app, &crate::adapter::SqliteAdapter)
}

pub fn emit_library(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    files.extend(library::emit_library_classes(app));
    files.extend(library::emit_library_class_decls(app));
    files
}

pub fn emit_with_adapter(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(main_ts::emit_main_ts(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    if !app.models.is_empty() {
        files.push(schema_sql::emit_schema_sql(app));
    }
    files.extend(model::emit_models(app));
    files.extend(library::emit_library_class_decls(app));
    if !app.controllers.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("src/http.ts"),
            content: HTTP_STUB_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.ts"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.ts"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.ts"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(controller::emit_ts_importmap(app));
        files.extend(controller::emit_controllers(app, adapter));
    }
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
        files.push(route_helpers::emit_route_helpers(app));
    }
    files.extend(view::emit_views(app));
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_ts_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_ts_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_ts_spec(tm, app));
        }
    }
    files
}

/// Emit a `LibraryClass` (a single class or mixin module from a
/// `runtime/ruby/*` file, with method signatures attached) as a
/// TypeScript class declaration — trailing newline included.
///
/// Surface choices:
///   * `parent: Some(StandardError)` → `extends Error` (TS's
///     equivalent). Other parents pass through verbatim.
///   * `parent: None` on a non-module → bare `class Foo`.
///   * `is_module: true` → bare `class Foo` for now (mixin semantics
///     are handled at the include site, not the definition site).
///   * Synthesized attr_reader pattern (zero-param method whose body
///     is `Ivar { name }` matching the method's own name) → emit as a
///     class field declaration; the read still works because callers
///     write `obj.foo` and TS resolves it to the field. Drops the
///     synthetic getter, which would have collided with the field.
///   * Synthesized attr_writer pattern (`name=` method that just
///     assigns the matching ivar) → drops likewise; the field
///     declaration above already supports `obj.foo = x`.
///   * `initialize` → `constructor`. Body uses TS's `this.x` for
///     ivars (already what `expr::emit_body` produces).
///   * `Class`-receiver methods → `static`.
///   * `include`s → emitted as a leading `// include: <Name>` comment;
///     real mixin support is deferred.
pub fn emit_library_class(class: &crate::dialect::LibraryClass) -> Result<String, String> {
    use crate::dialect::MethodReceiver;
    use crate::expr::ExprNode;

    let class_name = class.name.0.as_str();
    let mut out = String::new();

    // Identify synthesized attr_reader/writer pairs. The reader pattern:
    // `def foo; @foo; end` — zero params, body is `Ivar { name: foo }`.
    // The writer pattern: `def foo=(value); @foo = value; end`.
    let is_attr_reader = |m: &crate::dialect::MethodDef| -> bool {
        if !matches!(m.receiver, MethodReceiver::Instance) || !m.params.is_empty() {
            return false;
        }
        match &*m.body.node {
            ExprNode::Ivar { name } => name.as_str() == m.name.as_str(),
            _ => false,
        }
    };
    let is_attr_writer = |m: &crate::dialect::MethodDef| -> bool {
        if !matches!(m.receiver, MethodReceiver::Instance) || m.params.len() != 1 {
            return false;
        }
        let mname = m.name.as_str();
        if !mname.ends_with('=') {
            return false;
        }
        let attr = &mname[..mname.len() - 1];
        match &*m.body.node {
            ExprNode::Assign { target: crate::expr::LValue::Ivar { name }, value } => {
                if name.as_str() != attr {
                    return false;
                }
                matches!(
                    &*value.node,
                    ExprNode::Var { name, .. } if name.as_str() == m.params[0].name.as_str()
                )
            }
            _ => false,
        }
    };

    // Collect field declarations (from synthesized attr_readers — the
    // reader carries the type via its `() -> T` signature).
    let mut fields: Vec<(String, String)> = Vec::new();
    for m in &class.methods {
        if is_attr_reader(m) {
            let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() else {
                return Err(format!(
                    "class `{}` field `{}`: signature missing or not Ty::Fn",
                    class_name, m.name
                ));
            };
            fields.push((m.name.as_str().to_string(), ts_ty(ret)));
        }
    }

    // Class header. Parent: StandardError → Error special-case;
    // everything else passes through. Modules emit as classes for now;
    // include-as-mixin is deferred.
    let parent = class.parent.as_ref().map(|p| match p.0.as_str() {
        "StandardError" => "Error".to_string(),
        other => other.to_string(),
    });
    match &parent {
        Some(p) => writeln!(out, "export class {class_name} extends {p} {{").unwrap(),
        None => writeln!(out, "export class {class_name} {{").unwrap(),
    }

    if !class.includes.is_empty() {
        for inc in &class.includes {
            writeln!(out, "  // include: {}", inc.0.as_str()).unwrap();
        }
    }

    let mut wrote_fields = false;
    for (name, ty) in &fields {
        writeln!(out, "  {name}: {ty};").unwrap();
        wrote_fields = true;
    }

    let methods_to_emit: Vec<&crate::dialect::MethodDef> = class
        .methods
        .iter()
        .filter(|m| !is_attr_reader(m) && !is_attr_writer(m))
        .collect();

    if wrote_fields && !methods_to_emit.is_empty() {
        writeln!(out).unwrap();
    }

    let mut first = true;
    for m in methods_to_emit {
        if !first {
            writeln!(out).unwrap();
        }
        first = false;
        let body_str = emit_class_member(m)?;
        for line in body_str.lines() {
            if line.is_empty() {
                writeln!(out).unwrap();
            } else {
                writeln!(out, "  {line}").unwrap();
            }
        }
    }

    out.push_str("}\n");
    Ok(out)
}

/// Emit the body of a `constructor` from an `initialize` method's
/// `Expr`. Floats top-level `super(...)` calls to the front so TS's
/// strict-derived-class rule (no `this` access before super) holds
/// even when the source Ruby wrote `@x = arg; super(...)`.
///
/// Only top-level Seq elements are reordered; super calls nested
/// inside conditionals or other expressions stay where they are.
/// Framework Ruby's constructors use the simple `seq with super at
/// some position` shape; deeper nesting hasn't appeared yet.
fn emit_constructor_body(body: &crate::expr::Expr, return_ty: &Ty) -> String {
    use crate::expr::{Expr, ExprNode};

    let exprs: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };

    let (supers, rest): (Vec<&Expr>, Vec<&Expr>) = exprs
        .into_iter()
        .partition(|e| matches!(*e.node, ExprNode::Super { .. }));

    if supers.is_empty() {
        return expr::emit_body(body, return_ty);
    }

    let mut reordered_exprs: Vec<Expr> = Vec::new();
    for s in supers {
        reordered_exprs.push((*s).clone());
    }
    for r in rest {
        reordered_exprs.push((*r).clone());
    }
    let reordered = Expr::new(body.span, ExprNode::Seq { exprs: reordered_exprs });
    expr::emit_body(&reordered, return_ty)
}

/// Emit one `MethodDef` as a class member (instance method, static,
/// or constructor). Mirrors `emit_method` but without the `export
/// function` wrapper. Used by `emit_library_class`.
fn emit_class_member(m: &crate::dialect::MethodDef) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    let sig = m.signature.as_ref().ok_or_else(|| {
        format!("emit_class_member: method `{}` has no signature", m.name)
    })?;
    let Ty::Fn { params: sig_params, ret, .. } = sig else {
        return Err(format!("method `{}`: signature is not Ty::Fn", m.name));
    };
    if sig_params
        .iter()
        .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
        .count()
        != m.params.len()
    {
        return Err(format!(
            "method `{}`: signature/param arity mismatch",
            m.name
        ));
    }

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_params.iter().filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block)))
        .map(|(name, p)| {
            format!("{}: {}", escape_reserved(name.as_str()), ts_ty(&p.ty))
        })
        .collect();

    let mut out = String::new();
    let raw_name = m.name.as_str();
    let mname = crate::emit::typescript::library::sanitize_identifier(raw_name);
    let is_constructor =
        raw_name == "initialize" && matches!(m.receiver, MethodReceiver::Instance);

    // Rewrite the body for class context: bare `Send { recv: None }`
    // gets `SelfRef` so it emits as `this.method(...)` instead of a
    // dangling `method(...)`. Constructors keep `Super { args }` as-is
    // (TS spells parent-constructor calls as `super(args)`, not
    // `super.initialize(args)`); other methods get the
    // `super.<method>(...)` rewrite.
    let rewritten = if is_constructor {
        crate::emit::typescript::library::rewrite_for_constructor(&m.body)
    } else {
        crate::emit::typescript::library::rewrite_for_class_method(&m.body, raw_name)
    };

    // initialize → constructor (no return annotation; TS forbids one).
    // TS strict mode also requires `super(...)` to precede any `this.x`
    // access in derived-class constructors. The Ruby source typically
    // writes `@x = arg; super(msg)` (Ruby allows either order); we
    // reorder so super-calls float to the top of the constructor body.
    let body = if is_constructor {
        emit_constructor_body(&rewritten, ret)
    } else {
        expr::emit_body(&rewritten, ret)
    };

    if is_constructor {
        writeln!(out, "constructor({}) {{", param_list.join(", ")).unwrap();
    } else {
        let prefix = if matches!(m.receiver, MethodReceiver::Class) {
            "static "
        } else {
            ""
        };
        let ret_s = ts_ty(ret);
        writeln!(
            out,
            "{prefix}{}({}): {} {{",
            mname,
            param_list.join(", "),
            ret_s
        )
        .unwrap();
    }
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    Ok(out)
}

/// Emit a list of typed `MethodDef`s — produced by
/// `parse_methods_with_rbs` from a whole `.rb` + `.rbs` pair — as a
/// single TypeScript module file (trailing newline included).
///
/// Surface choice: when every method is `MethodReceiver::Class` (i.e.
/// the source was a module of `def self.*` helpers, like
/// `runtime/ruby/inflector.rb`), each method emits as a standalone
/// `export function`. The Ruby module name is absorbed into the import
/// path on the calling side. This matches the existing hand-written
/// shape (e.g. `export function pluralize` in
/// `runtime/typescript/view_helpers.ts`).
///
/// Other surface forms (mixin module → class with instance methods,
/// concrete class with state) are deferred to follow-up work; the
/// helper rejects them rather than emit half-correctly.
pub fn emit_module(methods: &[crate::dialect::MethodDef]) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    if methods.is_empty() {
        return Ok(String::new());
    }
    if !methods.iter().all(|m| matches!(m.receiver, MethodReceiver::Class)) {
        return Err(format!(
            "emit_module: only all-class-method modules supported so far; \
             saw mixed/instance methods (first instance: `{}`)",
            methods
                .iter()
                .find(|m| matches!(m.receiver, MethodReceiver::Instance))
                .map(|m| m.name.as_str())
                .unwrap_or("<none>"),
        ));
    }

    let mut out = String::new();
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&emit_method(m));
    }
    Ok(out)
}

/// Map a Ruby identifier to a safe TS parameter name. Ruby and TS
/// both allow most snake_case names verbatim; the divergence is
/// keywords. Each name in the list below is reserved in TS but
/// commonly used as a Rails-side method/keyword arg (`default` in
/// `params.fetch(:k, default)`, `with` in `validates_format_of(... with:)`).
/// Suffix with `_` rather than rename — preserves the original word
/// while clearing the reserved-word collision.
fn escape_reserved(name: &str) -> String {
    matches!(
        name,
        "default"
            | "with"
            | "function"
            | "class"
            | "for"
            | "let"
            | "const"
            | "var"
            | "return"
            | "switch"
            | "case"
            | "if"
            | "else"
            | "while"
            | "do"
            | "yield"
            | "delete"
            | "new"
            | "this"
            | "super"
            | "true"
            | "false"
            | "null"
            | "void"
            | "typeof"
            | "instanceof"
    )
    .then(|| format!("{name}_"))
    .unwrap_or_else(|| name.to_string())
}

/// Emit a typed `MethodDef` as a standalone exported TypeScript
/// function (trailing newline included). Requires `signature` to be
/// populated — `parse_methods_with_rbs` does this. Used by the
/// runtime-extraction pipeline.
pub fn emit_method(m: &crate::dialect::MethodDef) -> String {
    let sig = m
        .signature
        .as_ref()
        .expect("emit_method requires a signature");
    let Ty::Fn { params: sig_params, ret, .. } = sig else {
        panic!("signature is not Ty::Fn");
    };
    assert_eq!(
        sig_params.len(),
        m.params.len(),
        "method `{}`: signature/param arity mismatch",
        m.name
    );

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_params.iter())
        .map(|(name, p)| format!("{}: {}", name, ts_ty(&p.ty)))
        .collect();

    let ret_s = ts_ty(ret);
    let body = expr::emit_body(&m.body, ret);

    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        m.name,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}
