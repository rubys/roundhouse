//! TypeScript emitter — kind-agnostic LibraryClass walker.
//!
//! Phase B of the rewrite (2026-04-30): the emitter no longer knows
//! about views, controllers, models, schema, routes, or fixtures as
//! distinct output kinds. Every input flows through the lowerer
//! pipeline into `LibraryClass` and is rendered by
//! `library::emit_class_file`. Per-target surface = `expr.rs` (Expr →
//! TS syntax) + `ty.rs` (Ty → TS type) + `library.rs` (LibraryClass
//! walker) + ecosystem files (`package.json`, `tsconfig.json`,
//! `juntos.ts` runtime stub).
//!
//! Outputs not yet covered: controllers, schema, routes, importmap,
//! fixtures, specs. Each is a missing `*_to_library` lowerer (see
//! `project_universal_post_lowering_ir`); when the lowerer lands the
//! output joins the walker without changes here.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");
const MINITEST_RUNTIME_SOURCE: &str = include_str!("../../runtime/typescript/minitest.ts");

mod expr;
mod library;
mod naming;
mod package;
mod ty;

pub use ty::{ts_return_ty, ts_ty};

/// Emit a TypeScript project for `app`. Every artifact (models,
/// views, controllers, fixtures, tests, schema) flows through the
/// universal walker. A single shared class registry is threaded
/// through all lowerings so cross-class dispatch (`Article.find(...)`
/// from a controller body, `ArticlesFixtures.one()` from a test)
/// types end-to-end.
pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });

    // ── Lowering pipeline ───────────────────────────────────────────
    // Order matters because each step's output feeds the next's
    // shared registry. Views are lowered twice — once preliminarily
    // (without model knowledge) so models can dispatch on Views::*,
    // then again with the full model registry so view bodies can
    // dispatch on models.

    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = library::extras_from_lcs(&preliminary_views);

    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry(
        &app.models,
        &app.schema,
        view_extras,
    );

    let view_lcs = crate::lower::lower_views_to_library_classes(
        &app.views,
        app,
        model_registry.clone().into_iter().collect(),
    );

    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(library::extras_from_lcs(&view_lcs));
    let controller_lcs = crate::lower::lower_controllers_to_library_classes(
        &app.controllers,
        controller_extras.clone(),
    );

    let fixture_lcs = crate::lower::lower_fixtures_to_library_classes(app);

    let test_lcs = if app.test_modules.is_empty() {
        Vec::new()
    } else {
        let mut test_extras = controller_extras;
        test_extras.extend(library::extras_from_lcs(&controller_lcs));
        test_extras.extend(library::extras_from_lcs(&fixture_lcs));
        crate::lower::lower_test_modules_to_library_classes(
            &app.test_modules,
            &app.fixtures,
            &app.models,
            test_extras,
        )
    };

    // ── Emit ────────────────────────────────────────────────────────

    if let Some(schema_lc) = crate::lower::lower_schema_to_library_class(&app.schema) {
        files.push(library::emit_class_file(
            &schema_lc,
            app,
            PathBuf::from("src/schema.ts"),
        ));
    }

    for lc in &model_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/models/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    // view_lcs is in the same order as app.views (the bulk lowerer
    // preserves declaration order); pair them to recover the
    // per-template output path.
    for (view, lc) in app.views.iter().zip(view_lcs.iter()) {
        let out_path = view_output_path(view.name.as_str());
        files.push(library::emit_class_file(lc, app, out_path));
    }

    for lc in &controller_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/controllers/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    for lc in &app.library_classes {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/models/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    for lc in &fixture_lcs {
        let stem = fixture_file_stem(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("test/fixtures/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    if !test_lcs.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/_runtime/minitest.ts"),
            content: MINITEST_RUNTIME_SOURCE.to_string(),
        });
        for lc in &test_lcs {
            let stem = test_file_stem(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("test/{stem}.test.ts"));
            let mut emitted = library::emit_class_file(lc, app, out_path.clone());
            emitted.content.push('\n');
            emitted.content.push_str(&format!(
                "import {{ discover_tests }} from \"./_runtime/minitest.js\";\n\
                 discover_tests({});\n",
                lc.name.0.as_str(),
            ));
            files.push(emitted);
        }
    }

    files
}

/// Map a view name (`articles/index`, `articles/_article`,
/// `layouts/application`) to the output path under `app/views/`.
fn view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("app/views/{view_name}.ts"))
}

/// `ArticleTest` → `article` (strip Test suffix, snake_case). Used
/// for the `test/<stem>.test.ts` output path so the file name reads
/// naturally without redundant `_test_test`.
fn test_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Test").unwrap_or(class_name);
    crate::naming::snake_case(stem)
}

/// `ArticlesFixtures` → `articles` (strip Fixtures suffix, snake_case).
fn fixture_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Fixtures").unwrap_or(class_name);
    crate::naming::snake_case(stem)
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
    use crate::dialect::{AccessorKind, MethodReceiver};

    let class_name = class.name.0.as_str();
    let mut out = String::new();

    // Identify attribute readers/writers by the lowerer-recorded
    // `kind` field rather than pattern-matching the body — the
    // lowerer knows by construction (`synth_attr_reader`,
    // `synth_attr_writer`, `attr_*` ingest), so the IR carries the
    // fact directly. Restricted to instance receivers here because
    // class-receiver attribute accessors don't have an established
    // TS rendering pattern yet.
    let is_attr_reader = |m: &crate::dialect::MethodDef| -> bool {
        matches!(m.kind, AccessorKind::AttributeReader)
            && matches!(m.receiver, MethodReceiver::Instance)
            && m.params.is_empty()
    };
    let is_attr_writer = |m: &crate::dialect::MethodDef| -> bool {
        matches!(m.kind, AccessorKind::AttributeWriter)
            && matches!(m.receiver, MethodReceiver::Instance)
            && m.params.len() == 1
    };

    // Collect field declarations (from synthesized attr_readers — the
    // reader carries the type via its `() -> T` signature; body type
    // is the next-best source; final fallback is `any`).
    let mut fields: Vec<(String, String)> = Vec::new();
    for m in &class.methods {
        if is_attr_reader(m) {
            let ty = match m.signature.as_ref() {
                Some(Ty::Fn { ret, .. }) => ts_ty(ret),
                _ => m.body.ty.as_ref().map(ts_ty).unwrap_or_else(|| "any".to_string()),
            };
            fields.push((m.name.as_str().to_string(), ty));
        }
    }

    // Class header. Parent translation:
    //   - `StandardError` → `Error` (TS builtin)
    //   - `ActiveRecord::Base` → `ActiveRecord` (juntos export)
    //   - Other qualified names: last segment (Ruby's `Foo::Bar` → TS
    //     `Bar` after import)
    // Modules emit as classes for now; include-as-mixin is deferred.
    let parent = class.parent.as_ref().map(|p| {
        let raw = p.0.as_str();
        match raw {
            "StandardError" => "Error".to_string(),
            "ActiveRecord::Base" => "ActiveRecord".to_string(),
            // Test parents — runtime adapter exports both names.
            "ActiveSupport::TestCase" | "ActionDispatch::IntegrationTest" => "TestCase".to_string(),
            "Minitest::Test" => "Test".to_string(),
            // Controller base — runtime exports `Base`; the import is
            // aliased in render_imports so the extends clause reads
            // `extends ActionControllerBase`.
            "ActionController::Base" | "ActionController::API" => {
                "ActionControllerBase".to_string()
            }
            _ => raw.rsplit("::").next().unwrap_or(raw).to_string(),
        }
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
        .filter(|m| {
            // Operator-method names (`[]`, `[]=`) aren't valid TS
            // method identifiers. Real fix: rewrite call sites to
            // call `.get(...)` / `.set(...)` AND emit the method
            // bodies under those names. Until both halves land,
            // skipping prevents the file from being syntactically
            // invalid TypeScript. The bodies are lost on the
            // generated side but are still readable in the
            // source `.rb`.
            !matches!(m.name.as_str(), "[]" | "[]=")
        })
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
/// or constructor). Uses signature when present (typed params + ret);
/// falls back to body.ty for return and `any` for params when not
/// (lowered models don't populate signatures yet).
fn emit_class_member(m: &crate::dialect::MethodDef) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    // Pull (param-types, return-type) from signature when available.
    let (sig_param_tys, ret_ty): (Vec<Ty>, Ty) = match m.signature.as_ref() {
        Some(Ty::Fn { params: sig_params, ret, .. }) => {
            let non_block: Vec<&crate::ty::Param> = sig_params
                .iter()
                .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                .collect();
            if non_block.len() != m.params.len() {
                return Err(format!(
                    "method `{}`: signature/param arity mismatch ({} vs {})",
                    m.name,
                    non_block.len(),
                    m.params.len(),
                ));
            }
            (non_block.iter().map(|p| p.ty.clone()).collect(), (**ret).clone())
        }
        _ => (
            m.params.iter().map(|_| Ty::Untyped).collect(),
            m.body.ty.clone().unwrap_or(Ty::Nil),
        ),
    };

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_param_tys.iter())
        .map(|(name, ty)| format!("{}: {}", escape_reserved(name.as_str()), ts_ty(ty)))
        .collect();

    let mut out = String::new();
    let raw_name = m.name.as_str();
    let mname = crate::emit::typescript::library::sanitize_identifier(raw_name);
    let is_constructor =
        raw_name == "initialize" && matches!(m.receiver, MethodReceiver::Instance);

    let rewritten = if is_constructor {
        crate::emit::typescript::library::rewrite_for_constructor(&m.body)
    } else {
        crate::emit::typescript::library::rewrite_for_class_method(&m.body, raw_name)
    };

    let body = if is_constructor {
        emit_constructor_body(&rewritten, &ret_ty)
    } else {
        expr::emit_body(&rewritten, &ret_ty)
    };

    if is_constructor {
        writeln!(out, "constructor({}) {{", param_list.join(", ")).unwrap();
    } else {
        let prefix = if matches!(m.receiver, MethodReceiver::Class) {
            "static "
        } else {
            ""
        };
        let ret_s = ts_return_ty(&ret_ty);
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

/// Map a Ruby identifier to a safe TS parameter name. Each name in
/// the list below is reserved in TS but commonly used as a Rails-side
/// method/keyword arg.
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
/// function (trailing newline included). Used by the
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

    let ret_s = ts_return_ty(ret);
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
