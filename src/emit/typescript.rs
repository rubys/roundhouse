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

/// Framework runtime files inlined at the canonical `src/<name>.ts`
/// path. Internal cross-imports use `./<name>.js` so emitting all of
/// them under the same flat directory satisfies module resolution.
/// Excludes `juntos.ts` (mapped via tsconfig path alias) and
/// `minitest.ts` (lives under `test/_runtime/`); both are handled
/// separately by their own emit slots.
const RUNTIME_FILES: &[(&str, &str)] = &[
    (
        "src/action_controller_base.ts",
        include_str!("../../runtime/typescript/action_controller_base.ts"),
    ),
    (
        "src/active_record_base.ts",
        include_str!("../../runtime/typescript/active_record_base.ts"),
    ),
    (
        "src/broadcasts.ts",
        include_str!("../../runtime/typescript/broadcasts.ts"),
    ),
    ("src/errors.ts", include_str!("../../runtime/typescript/errors.ts")),
    ("src/http.ts", include_str!("../../runtime/typescript/http.ts")),
    (
        "src/inflector.ts",
        include_str!("../../runtime/typescript/inflector.ts"),
    ),
    (
        "src/parameters.ts",
        include_str!("../../runtime/typescript/parameters.ts"),
    ),
    ("src/router.ts", include_str!("../../runtime/typescript/router.ts")),
    ("src/server.ts", include_str!("../../runtime/typescript/server.ts")),
    (
        "src/test_support.ts",
        include_str!("../../runtime/typescript/test_support.ts"),
    ),
    (
        "src/validations.ts",
        include_str!("../../runtime/typescript/validations.ts"),
    ),
    (
        "src/view_helpers.ts",
        include_str!("../../runtime/typescript/view_helpers.ts"),
    ),
    (
        "src/view_helpers_generated.ts",
        include_str!("../../runtime/typescript/view_helpers_generated.ts"),
    ),
];

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

    // Framework runtime files. These are hand-written (or transpiled
    // from runtime/ruby/) and inlined into the output so the emitted
    // app has no external dependency on roundhouse itself. Layout:
    // flat under `src/`; internal cross-imports use `./<name>.js`.
    for (path, content) in RUNTIME_FILES {
        files.push(EmittedFile {
            path: PathBuf::from(*path),
            content: (*content).to_string(),
        });
    }

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

    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    let route_helper_extras = library::extras_from_funcs(&route_helper_funcs);

    // Collect controller `permit(...)` declarations once so the model
    // lowerer can synthesize `from_params(p: <Resource>Params)` factories
    // matching the permitted-fields list. The same specs feed the
    // controller lowerer below — both call sites need the same view
    // of the controller-derived metadata.
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs_simple: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();
    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
        &params_specs_simple,
    );

    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    view_lower_extras.extend(route_helper_extras.clone());
    let view_lcs = crate::lower::lower_views_to_library_classes(
        &app.views,
        app,
        view_lower_extras,
    );

    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(library::extras_from_lcs(&view_lcs));
    controller_extras.extend(route_helper_extras);
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

    let schema_funcs = crate::lower::lower_schema_to_library_functions(&app.schema);
    if !schema_funcs.is_empty() {
        files.push(library::emit_module_file(
            &schema_funcs,
            app,
            PathBuf::from("src/schema.ts"),
        ));
    }

    if !route_helper_funcs.is_empty() {
        files.push(library::emit_module_file(
            &route_helper_funcs,
            app,
            PathBuf::from("app/route_helpers.ts"),
        ));
    }

    let routes_dispatch_funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    if !routes_dispatch_funcs.is_empty() {
        files.push(library::emit_module_file(
            &routes_dispatch_funcs,
            app,
            PathBuf::from("app/routes.ts"),
        ));
    }

    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    if !importmap_funcs.is_empty() {
        files.push(library::emit_module_file(
            &importmap_funcs,
            app,
            PathBuf::from("app/importmap.ts"),
        ));
    }

    let has_seeds = app.seeds.is_some();
    let seeds_funcs = crate::lower::lower_seeds_to_library_functions(app);
    if !seeds_funcs.is_empty() {
        files.push(library::emit_module_file(
            &seeds_funcs,
            app,
            PathBuf::from("db/seeds.ts"),
        ));
    }

    // Synthesized siblings (`<Model>Row` from models, `<Resource>Params`
    // from controllers) carry an `origin` tag. Combine both into one
    // list so render_imports recognizes them as model-style imports —
    // they all live in `app/models/` regardless of which lowerer
    // produced them.
    let mut synthesized_names: Vec<String> = model_lcs
        .iter()
        .chain(controller_lcs.iter())
        .filter(|lc| lc.origin.is_some())
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    synthesized_names.sort();
    synthesized_names.dedup();
    for lc in &model_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/models/{stem}.ts"));
        files.push(library::emit_class_file_with_synthesized(
            lc,
            app,
            out_path,
            &synthesized_names,
        ));
    }

    // Views: flatten the per-template LibraryClasses into
    // LibraryFunctions and emit one function per file. The body-typer
    // registry above (`view_extras` / `extras_from_lcs(&view_lcs)`)
    // still uses the class shape so cross-class dispatch
    // (`Views::Articles.article(x)`) types correctly without a
    // parallel registry. The class-vs-function choice is purely an
    // emit-side surface decision.
    let view_funcs = crate::lower::flatten_lcs_to_functions(&view_lcs);
    for (view, func) in app.views.iter().zip(view_funcs.iter()) {
        let out_path = view_output_path(view.name.as_str());
        files.push(library::emit_function_file(func, app, out_path));
    }
    if !view_funcs.is_empty() {
        files.push(library::emit_views_aggregator(&app.views, &view_funcs));
    }

    // Synthesized `<Resource>Params` classes ride in `controller_lcs`
    // (origin tagged); route those to `app/models/` rather than
    // `app/controllers/`. Use the combined `synthesized_names` so a
    // controller body's reference to a Row class (or any other
    // synthesized class) resolves uniformly.
    for lc in &controller_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = if lc.origin.is_some() {
            PathBuf::from(format!("app/models/{stem}.ts"))
        } else {
            PathBuf::from(format!("app/controllers/{stem}.ts"))
        };
        files.push(library::emit_class_file_with_synthesized(
            lc,
            app,
            out_path,
            &synthesized_names,
        ));
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

    files.push(emit_main_ts(app, has_seeds));

    files
}

/// Hand-written `main.ts` shell. Wires together the generated
/// schema, optional seeds, and the runtime's `startServer`. Routes
/// are still TODO — the dispatch table needs a separate lowerer
/// that registers controllers with `Router`. For now, importing
/// each controller pulls them in for side-effect (their class
/// definitions; they'll be wired up once the route table emit
/// lands).
fn emit_main_ts(app: &App, has_seeds: bool) -> EmittedFile {
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse.\n");
    s.push_str("import { startServer } from \"./src/server.js\";\n");
    if !app.schema.tables.is_empty() {
        s.push_str("import { Schema } from \"./src/schema.js\";\n");
    }
    if has_seeds {
        s.push_str("import { Seeds } from \"./db/seeds.js\";\n");
    }
    for c in &app.controllers {
        let stem = crate::naming::snake_case(c.name.0.as_str());
        s.push_str(&format!(
            "import \"./app/controllers/{stem}.js\";\n"
        ));
    }
    s.push('\n');
    s.push_str("await startServer({\n");
    if app.schema.tables.is_empty() {
        s.push_str("  schemaStatements: [],\n");
    } else {
        s.push_str("  schemaStatements: Schema.statements(),\n");
    }
    if has_seeds {
        s.push_str("  seeds: () => Seeds.run(),\n");
    }
    s.push_str("});\n");
    EmittedFile {
        path: PathBuf::from("main.ts"),
        content: s,
    }
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
        matches!(m.kind, AccessorKind::AttributeReader) && m.params.is_empty()
    };
    let is_attr_writer = |m: &crate::dialect::MethodDef| -> bool {
        matches!(m.kind, AccessorKind::AttributeWriter) && m.params.len() == 1
    };

    // Collect field declarations (from synthesized attr_readers — the
    // reader carries the type via its `() -> T` signature; body type
    // is the next-best source; final fallback is `any`). Class-level
    // attr_accessors (from `class << self; attr_accessor :x; end`)
    // become `static x: T;` field declarations; instance-level become
    // `x: T;`. Either form's setter is suppressed in favor of plain
    // assignment to the field.
    let mut fields: Vec<(String, String, bool)> = Vec::new(); // (name, ty, is_static)
    let mut field_names_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &class.methods {
        if is_attr_reader(m) {
            let ty = match m.signature.as_ref() {
                Some(Ty::Fn { ret, .. }) => ts_ty(ret),
                _ => m.body.ty.as_ref().map(ts_ty).unwrap_or_else(|| "any".to_string()),
            };
            let is_static = matches!(m.receiver, MethodReceiver::Class);
            field_names_seen.insert(m.name.as_str().to_string());
            fields.push((m.name.as_str().to_string(), ty, is_static));
        }
    }

    // Pre-walk every instance-method body looking for `@ivar = …`
    // assignments that aren't already declared as attr_readers.
    // TypeScript strict-mode requires field declarations for
    // `this.foo` writes; controllers (which assign action-locals like
    // `@article` for the view to read) and tests (which assign
    // fixture-helpers at setup time) are the producers. Type comes
    // from the assignment RHS; falls back to `any` when the analyzer
    // didn't infer one.
    let mut ivar_assignments: indexmap::IndexMap<String, Ty> = indexmap::IndexMap::new();
    for m in &class.methods {
        if matches!(m.receiver, MethodReceiver::Instance) {
            collect_ivar_assignments(&m.body, &mut ivar_assignments);
        }
    }
    for (name, ty) in ivar_assignments {
        if field_names_seen.insert(name.clone()) {
            fields.push((name, ts_ty(&ty), false));
        }
    }

    // Class header. Parent translation:
    //   - `StandardError` → `Error` (TS builtin)
    //   - `ActiveRecord::Base` → `ActiveRecordBase` (transpiled, aliased
    //     from `Base` in src/active_record_base.ts via render_imports).
    //     The juntos-side `ApplicationRecord` was a parallel
    //     hand-written re-implementation; this redirect makes the
    //     transpiled framework Ruby the single source of truth and
    //     forces juntos's surface down to the per-target primitive
    //     layer (`project_active_record_layering`).
    //   - Other qualified names: last segment (Ruby's `Foo::Bar` → TS
    //     `Bar` after import)
    // Modules emit as classes for now; include-as-mixin is deferred.
    let parent = class.parent.as_ref().map(|p| {
        let raw = p.0.as_str();
        match raw {
            "StandardError" => "Error".to_string(),
            "ActiveRecord::Base" => "ActiveRecordBase".to_string(),
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
    // `include Mod` semantics: Ruby's mixin doesn't translate to TS
    // multiple inheritance. Single-include + no-parent collapses to
    // `extends <Mod>` so the included module's methods reach
    // subclasses through TS's inheritance chain. Other shapes
    // (include with explicit parent, multiple includes) still emit
    // a comment placeholder — they need the include-as-mixin pass.
    let synthesized_parent = if parent.is_none() && class.includes.len() == 1 {
        Some(class.includes[0].0.as_str().rsplit("::").next().unwrap().to_string())
    } else {
        None
    };
    let effective_parent = parent.as_deref().or(synthesized_parent.as_deref());
    match effective_parent {
        Some(p) => writeln!(out, "export class {class_name} extends {p} {{").unwrap(),
        None => writeln!(out, "export class {class_name} {{").unwrap(),
    }

    if synthesized_parent.is_none() && !class.includes.is_empty() {
        for inc in &class.includes {
            writeln!(out, "  // include: {}", inc.0.as_str()).unwrap();
        }
    }

    let mut wrote_fields = false;
    for (name, ty, is_static) in &fields {
        let prefix = if *is_static { "static " } else { "" };
        writeln!(out, "  {prefix}{name}: {ty};").unwrap();
        wrote_fields = true;
    }

    // `?`/`!` method-name suffixes get stripped on the way out
    // (`save!` → `save`, `valid?` → `valid`); when both forms exist
    // on the same class the sanitized names collide and TS rejects
    // the duplicate member. Drop the bang/predicate variant when a
    // plain-named twin exists — either as another method with the
    // same sanitized name (`save` vs `save!`) or as a field
    // declaration (`@persisted` ivar field collides with `persisted?`
    // sanitized to `persisted`). Predicate bodies that just read the
    // ivar (`def persisted?; @persisted; end`) are subsumed by the
    // field; callers reading `record.persisted?` sanitize to
    // `record.persisted` and get the field directly.
    let mut sanitized_seen: std::collections::HashSet<String> =
        field_names_seen.clone();
    for m in &class.methods {
        let raw = m.name.as_str();
        if !raw.ends_with('?') && !raw.ends_with('!') {
            sanitized_seen.insert(crate::emit::typescript::library::sanitize_identifier(raw));
        }
    }

    let methods_to_emit: Vec<&crate::dialect::MethodDef> = class
        .methods
        .iter()
        .filter(|m| !is_attr_reader(m) && !is_attr_writer(m))
        .filter(|m| {
            // Operator-method names (`[]`, `[]=`, `==`, …) aren't
            // valid TS method identifiers. TS lacks operator
            // overloading, so even renaming (`==` → `equals`) leaves
            // the bodies uncalled by the emitted code: comparison
            // sites lower to `===`, indexing lowers to `[]`, etc.
            // Skipping keeps the file syntactically valid; the
            // bodies remain readable in the source `.rb`.
            !matches!(
                m.name.as_str(),
                "[]" | "[]=" | "==" | "!=" | "<=>" | "<" | ">" | "<=" | ">="
                    | "<<" | ">>" | "+" | "-" | "*" | "/" | "%" | "**"
                    | "&" | "|" | "^" | "~" | "!" | "==="
            )
        })
        .filter(|m| {
            let raw = m.name.as_str();
            let stripped =
                crate::emit::typescript::library::sanitize_identifier(raw);
            // Drop the method whenever its sanitized name collides
            // with a field declaration (ivar OR attr_reader). The
            // common cases: `def errors; @errors ||= []; end`,
            // `def persisted?; @persisted; end` — bodies that just
            // accessor-expose an ivar are subsumed by the field.
            // Non-trivial colliders (rare) lose runtime semantics
            // here; surface those as a separate Ruby-source change
            // rather than emitting broken TS.
            if field_names_seen.contains(&stripped) {
                return false;
            }
            // Predicate/bang vs same-name plain-method twin (`save`
            // vs `save!`): keep the plain twin.
            if raw.ends_with('?') || raw.ends_with('!') {
                !sanitized_seen.contains(&stripped)
            } else {
                true
            }
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

    // Pull (param-types, kinds, return-type) from signature when
    // available. Kinds drive optional-param decoration: Ruby kwargs
    // with defaults (`def foo(x, status: 200)`) and explicit-optional
    // positionals (`def foo(x = nil)`) emit as TS `name?: T` so
    // call sites that omit them type-check. Without this, every
    // kwarg-default call (`render(html)` where Ruby has
    // `render(html, status: 200)`) trips TS2554.
    let (sig_param_tys, sig_param_optional, ret_ty): (Vec<Ty>, Vec<bool>, Ty) =
        match m.signature.as_ref() {
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
                let tys = non_block.iter().map(|p| p.ty.clone()).collect();
                let optionals = non_block
                    .iter()
                    .map(|p| {
                        matches!(
                            p.kind,
                            crate::ty::ParamKind::Optional
                                | crate::ty::ParamKind::Keyword { required: false }
                                | crate::ty::ParamKind::KeywordRest
                        )
                    })
                    .collect();
                (tys, optionals, (**ret).clone())
            }
            _ => (
                m.params.iter().map(|_| Ty::Untyped).collect(),
                m.params.iter().map(|_| false).collect(),
                m.body.ty.clone().unwrap_or(Ty::Nil),
            ),
        };

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_param_tys.iter())
        .zip(sig_param_optional.iter())
        .map(|((name, ty), optional)| {
            let opt_marker = if *optional { "?" } else { "" };
            format!(
                "{}{}: {}",
                escape_reserved(name.as_str()),
                opt_marker,
                ts_ty(ty)
            )
        })
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

/// Emit a `LibraryFunction` as a top-level `export function` (no
/// surrounding class). Body emission shares the param-typing /
/// return-typing / body-typing machinery with `emit_class_member`,
/// but the rewrite pass differs: free functions don't have `this`,
/// so bare Sends and Ivar references aren't injected with SelfRef.
pub fn emit_library_function(
    func: &crate::dialect::LibraryFunction,
) -> Result<String, String> {
    let (sig_param_tys, sig_param_optional, ret_ty): (Vec<Ty>, Vec<bool>, Ty) =
        match func.signature.as_ref() {
            Some(Ty::Fn { params: sig_params, ret, .. }) => {
                let non_block: Vec<&crate::ty::Param> = sig_params
                    .iter()
                    .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                    .collect();
                if non_block.len() != func.params.len() {
                    return Err(format!(
                        "function `{}`: signature/param arity mismatch ({} vs {})",
                        func.name,
                        non_block.len(),
                        func.params.len(),
                    ));
                }
                let tys = non_block.iter().map(|p| p.ty.clone()).collect();
                let optionals = non_block
                    .iter()
                    .map(|p| {
                        matches!(
                            p.kind,
                            crate::ty::ParamKind::Optional
                                | crate::ty::ParamKind::Keyword { required: false }
                                | crate::ty::ParamKind::KeywordRest
                        )
                    })
                    .collect();
                (tys, optionals, (**ret).clone())
            }
            _ => (
                func.params.iter().map(|_| Ty::Untyped).collect(),
                func.params.iter().map(|_| false).collect(),
                func.body.ty.clone().unwrap_or(Ty::Nil),
            ),
        };

    let param_list: Vec<String> = func
        .params
        .iter()
        .zip(sig_param_tys.iter())
        .zip(sig_param_optional.iter())
        .map(|((name, ty), optional)| {
            let opt_marker = if *optional { "?" } else { "" };
            format!(
                "{}{}: {}",
                escape_reserved(name.as_str()),
                opt_marker,
                ts_ty(ty)
            )
        })
        .collect();

    let raw_name = func.name.as_str();
    let mname = escape_for_function_name(raw_name);

    // Free-function rewrite: no SelfRef injection, no super rewrite —
    // bare Sends emit as plain function calls (resolved against
    // imports), and `super` doesn't apply since there's no inheritance.
    let rewritten = crate::emit::typescript::library::rewrite_for_free_function(&func.body);
    let body = expr::emit_body(&rewritten, &ret_ty);

    let ret_s = ts_return_ty(&ret_ty);
    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        mname,
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
/// Identifier escape applied to LibraryFunction names. Strips Ruby's
/// `?`/`!` suffixes via `sanitize_identifier`, then maps reserved
/// JS words (`new`, `default`, etc.) to a `name_` suffix form so the
/// emitted `export function <x>` parses.
pub(super) fn escape_for_function_name(raw: &str) -> String {
    escape_reserved(&crate::emit::typescript::library::sanitize_identifier(raw))
}

/// Walk an Expr collecting every `@ivar = value` assignment, keyed
/// by the ivar name. Later assignments overwrite earlier ones (keeps
/// the most-narrowed type when the body assigns the same ivar
/// multiple places). Used by `emit_library_class` to synthesize
/// `name: type;` field declarations for ivars that aren't otherwise
/// declared via attr_reader.
fn collect_ivar_assignments(
    e: &crate::expr::Expr,
    out: &mut indexmap::IndexMap<String, Ty>,
) {
    use crate::expr::{ExprNode, InterpPart, LValue};
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            // Type from the RHS, falling back to `any` (Ty::Untyped)
            // when the analyzer didn't infer one.
            let ty = value.ty.clone().unwrap_or(Ty::Untyped);
            out.insert(name.as_str().to_string(), ty);
            collect_ivar_assignments(value, out);
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_ivar_assignments(recv, out);
            }
            collect_ivar_assignments(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivar_assignments(r, out);
            }
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            collect_ivar_assignments(fun, out);
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivar_assignments(k, out);
                collect_ivar_assignments(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                collect_ivar_assignments(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_ivar_assignments(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivar_assignments(left, out);
            collect_ivar_assignments(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivar_assignments(value, out);
            collect_ivar_assignments(body, out);
        }
        ExprNode::Lambda { body, .. } => collect_ivar_assignments(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(then_branch, out);
            collect_ivar_assignments(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_ivar_assignments(scrutinee, out);
            for arm in arms {
                collect_ivar_assignments(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for sub in exprs {
                collect_ivar_assignments(sub, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_ivar_assignments(a, out);
            }
        }
        ExprNode::Raise { value } => collect_ivar_assignments(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            collect_ivar_assignments(expr, out);
            collect_ivar_assignments(fallback, out);
        }
        ExprNode::Return { value } => collect_ivar_assignments(value, out),
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_ivar_assignments(body, out);
            for r in rescues {
                collect_ivar_assignments(&r.body, out);
            }
            if let Some(eb) = else_branch {
                collect_ivar_assignments(eb, out);
            }
            if let Some(ensure_b) = ensure {
                collect_ivar_assignments(ensure_b, out);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value {
                collect_ivar_assignments(v, out);
            }
        }
        ExprNode::MultiAssign { value, .. } => collect_ivar_assignments(value, out),
        ExprNode::While { cond, body, .. } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_ivar_assignments(b, out);
            }
            if let Some(e2) = end {
                collect_ivar_assignments(e2, out);
            }
        }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_ivar_assignments(a, out);
                }
            }
        }
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

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
