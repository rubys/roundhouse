//! Python universal-IR overlay — the live dispatch path (the
//! strangler that replaced the per-artifact controller emit and the
//! `CtrlWalker` trait it implemented; the per-artifact model/view
//! emit still ships alongside and retires next).
//!
//! Mirrors the go/elixir overlay pattern: lower the whole app to
//! `LibraryClass` shape with the same call chain the Go emitter runs,
//! then render each family through `library::emit_library_class` —
//! the walker already proven on the LIVE_FRAMEWORK runtime units.
//! Layout is one file per family under `app/v2/` (Python needs no
//! per-class files, and family files keep imports trivial):
//!
//!   app/v2/models.py         — Application­Record + models + Row classes
//!   app/v2/views.py          — per-resource view classes + `Views` namespace
//!   app/v2/controllers.py    — controllers + strong-param classes
//!   app/v2/route_helpers.py  — `RouteHelpers` module class
//!   app/v2/dispatch.py       — controller registry + request dispatch
//!
//! The models↔views cycle (model broadcast bodies render partials via
//! `Views.*`) resolves by the established module trick (see
//! src/emit/python/model.rs): views.py imports model names at its
//! top; models.py binds `Views` at its BOTTOM, after every class is
//! defined, so whichever module loads first completes.
//!
//! Shipped by `emit()`; `server.py` and TestClient route every
//! request through `dispatch.handle`. Gated by
//! `tests/python_controller_library_emit.rs` (per-family walker
//! inventory + whole-set py_compile + live index-request drivers).
//! See docs/python-overlay-plan.md.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::library::emit_library_class;
use crate::App;
use crate::dialect::LibraryClass;
use crate::emit::EmittedFile;

/// The lowered LC families the overlay renders. Public so the
/// inventory test can report per-family.
pub struct OverlayLcs {
    pub controllers: Vec<LibraryClass>,
    pub models: Vec<LibraryClass>,
    pub views: Vec<LibraryClass>,
    pub route_helpers: Option<LibraryClass>,
    pub importmap: Option<LibraryClass>,
    pub route_table: Option<LibraryClass>,
    pub fixtures: Vec<LibraryClass>,
    pub tests: Vec<LibraryClass>,
}

/// Lower the app to overlay shape — the same assembly the Go emitter
/// runs (src/emit/go.rs): permit() specs feed model `from_params`
/// synthesis; the model registry carries the AR baseline class
/// methods the controller body-typer needs; the association graph
/// feeds the nested-resource rewrites.
pub fn lower_overlay(app: &App) -> OverlayLcs {
    let params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let (models, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &params_specs,
    );
    let model_extras: Vec<_> = model_registry.into_iter().collect();
    let assocs = crate::lower::model_associations::compute_association_graph(app);
    let controllers = crate::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        model_extras.clone(),
        Some(&app.schema),
        &app.views,
        &assocs,
    );
    let mut views =
        crate::lower::lower_views_to_library_classes(&app.views, app, model_extras.clone());
    // JBuilder (`*.json.jbuilder`) views produce the `<action>_json`
    // methods on the same `Views::<Resource>` classes — the merge per
    // resource happens in emit_views. Mirrors the Go emitter.
    views.extend(crate::lower::lower_jbuilder_to_library_classes(
        &app.views,
        app,
        model_extras,
    ));
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    let route_helpers = if route_helper_funcs.is_empty() {
        None
    } else {
        Some(crate::lower::module_funcs_to_library_class(
            "RouteHelpers",
            &route_helper_funcs,
        ))
    };
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    let importmap = if importmap_funcs.is_empty() {
        None
    } else {
        Some(crate::lower::module_funcs_to_library_class(
            "Importmap",
            &importmap_funcs,
        ))
    };
    let dispatch_funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    let route_table = if dispatch_funcs.is_empty() {
        None
    } else {
        Some(crate::lower::module_funcs_to_library_class(
            "RouteTable",
            &dispatch_funcs,
        ))
    };
    // Fixtures + test modules, assembled the way the Ruby (spinel)
    // emit does: fixture-LC infos join the model registry as extras so
    // test bodies resolve `articles(:one)`-rewritten calls and model
    // chains alike.
    let fixtures = crate::lower::lower_fixtures_to_library_classes(app);
    let (_, test_model_registry) =
        crate::lower::lower_models_with_registry(&app.models, &app.schema, Vec::new());
    let fixture_extras: Vec<_> = fixtures
        .iter()
        .map(|lc| (lc.name.clone(), crate::lower::class_info_from_library_class(lc)))
        .chain(test_model_registry)
        .collect();
    let tests = crate::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        fixture_extras,
        &crate::lower::routes::helper_id_segments(app),
    );
    OverlayLcs { controllers, models, views, route_helpers, importmap, route_table, fixtures, tests }
}

/// Emit the overlay file set. Fails loudly on the first class the
/// walker can't render — the inventory test reports per-class.
pub fn emit_overlay_files(app: &App) -> Result<Vec<EmittedFile>, String> {
    let lcs = lower_overlay(app);
    let mut files = Vec::new();
    files.push(file("app/v2/__init__.py", String::new()));
    files.push(file("app/v2/models.py", emit_models(&lcs.models)?));
    files.push(file("app/v2/views.py", emit_views(&lcs.views, &lcs.models, lcs.importmap.is_some())?));
    files.push(file(
        "app/v2/controllers.py",
        emit_controllers(&lcs.controllers, &lcs.models)?,
    ));
    if let Some(rh) = &lcs.route_helpers {
        files.push(file("app/v2/route_helpers.py", emit_module_class(rh)?));
    }
    if let Some(im) = &lcs.importmap {
        files.push(file("app/v2/importmap.py", emit_module_class(im)?));
    }
    if let Some(rt) = &lcs.route_table {
        // RouteTable.root()/.table() build the transpiled
        // ActionDispatch Route objects the transpiled Router.match
        // consumes; the namespace collapse renders
        // `ActionDispatch::Router::Route.new(...)` as `Route(...)`.
        let mut out = String::from(HEADER);
        out.push_str("\nfrom app.router import Route\n\n");
        out.push_str(&emit_library_class(rt)?);
        files.push(file("app/v2/routes.py", out));
    }
    // The TRANSPILED view helpers, under the overlay's own path: the
    // legacy world ships hand-written `app/view_helpers.py` in place
    // of this unit (its degradations are noted at `LIVE_FRAMEWORK`),
    // so the overlay carries the transpiled one side-by-side until
    // Phase E retires the split.
    let vh = crate::runtime_loader::python_units_subset(
        &["app/view_helpers.py"],
        |_, classes| classes,
    )?;
    if let Some(unit) = vh.into_iter().next() {
        files.push(file("app/v2/view_helpers.py", unit.content));
    }
    if !lcs.fixtures.is_empty() {
        files.push(file("app/v2/fixtures.py", emit_fixtures(&lcs.fixtures, &lcs.models)?));
    }
    for lc in &lcs.tests {
        let (path, content) = emit_test_file(lc, &lcs)?;
        files.push(EmittedFile { path: PathBuf::from(path), content });
    }
    let has_layout = lcs
        .views
        .iter()
        .any(|lc| last_segment(lc.name.0.as_str()) == "Layouts");
    files.push(file(
        "app/v2/dispatch.py",
        emit_dispatch(&lcs.controllers, lcs.route_table.as_ref(), has_layout),
    ));
    Ok(files)
}

fn file(path: &str, content: String) -> EmittedFile {
    EmittedFile { path: PathBuf::from(path), content }
}

fn last_segment(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

/// Class-definition order: parents before subclasses (Python executes
/// class statements top to bottom). Stable within each layer.
fn parent_first(lcs: &[LibraryClass]) -> Vec<&LibraryClass> {
    let names: BTreeSet<&str> =
        lcs.iter().map(|lc| last_segment(lc.name.0.as_str())).collect();
    let mut remaining: Vec<&LibraryClass> = lcs.iter().collect();
    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|lc| {
            let parent_pending = lc.parent.as_ref().is_some_and(|p| {
                let p = last_segment(p.0.as_str());
                names.contains(p) && !emitted.contains(p)
            });
            if parent_pending {
                true
            } else {
                emitted.insert(last_segment(lc.name.0.as_str()));
                out.push(*lc);
                false
            }
        });
        assert!(remaining.len() < before, "parent cycle in overlay classes");
    }
    out
}

fn model_names(models: &[LibraryClass]) -> Vec<String> {
    models
        .iter()
        .map(|lc| last_segment(lc.name.0.as_str()).to_string())
        .collect()
}

/// Parents referenced by the family but defined nowhere in it. A
/// fixture without an explicit `ApplicationRecord` /
/// `ApplicationController` still subclasses the name (Rails treats
/// it as an implicit abstract subclass), so the family file aliases
/// each missing parent to the framework `Base` imported above.
fn missing_parent_aliases(lcs: &[LibraryClass]) -> String {
    let defined: BTreeSet<&str> =
        lcs.iter().map(|lc| last_segment(lc.name.0.as_str())).collect();
    let mut missing: Vec<&str> = lcs
        .iter()
        .filter_map(|lc| lc.parent.as_ref())
        .map(|p| last_segment(p.0.as_str()))
        .filter(|p| *p != "Base" && !defined.contains(p))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    let mut out = String::new();
    for name in missing {
        out.push_str(&format!("\n{name} = Base"));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

const HEADER: &str = "# Generated by Roundhouse (python overlay).\nfrom __future__ import annotations\n";

fn emit_models(models: &[LibraryClass]) -> Result<String, String> {
    let mut body = String::new();
    for lc in parent_first(models) {
        body.push('\n');
        body.push_str(&emit_library_class(lc)?);
    }
    let mut out = String::from(HEADER);
    // ActiveRecord's transpiled base — models subclass its `Base`.
    out.push_str("\nfrom app.active_record_base import Base\n");
    for (needle, import) in [
        ("Db.", "from app.db import Db\n"),
        ("RecordNotFound", "from app.errors import RecordNotFound\n"),
        ("RecordInvalid", "from app.errors import RecordInvalid\n"),
        // The temporal intrinsics (`ActiveSupport.db_now` et al)
        // render as `Roundhouse.RhDateTime.*` — same content-scan
        // import the legacy model emit makes.
        ("Roundhouse.RhDateTime", "from app.rh_datetime import Roundhouse\n"),
        // Turbo Stream broadcast hooks (`after_*_commit`).
        ("Broadcasts.", "from app.cable import Broadcasts\n"),
    ] {
        if body.contains(needle) {
            out.push_str(import);
        }
    }
    out.push_str(&missing_parent_aliases(models));
    out.push_str(&body);
    // Bottom import breaks the models↔views cycle (see module doc).
    if body.contains("Views.") {
        out.push_str("\nfrom app.v2.views import Views  # noqa: E402 — cycle-breaking bottom import\n");
    }
    Ok(out)
}

fn emit_views(
    views: &[LibraryClass],
    models: &[LibraryClass],
    has_importmap: bool,
) -> Result<String, String> {
    // The view lowering produces one LC per template, all named
    // `Views::<Resource>` — merge per resource so each class is
    // defined once with every template's method.
    let mut order: Vec<String> = Vec::new();
    let mut merged: Vec<LibraryClass> = Vec::new();
    for lc in views {
        let name = last_segment(lc.name.0.as_str()).to_string();
        match merged.iter_mut().find(|m| last_segment(m.name.0.as_str()) == name) {
            Some(m) => m.methods.extend(lc.methods.iter().cloned()),
            None => {
                order.push(name);
                merged.push(lc.clone());
            }
        }
    }

    let mut body = String::new();
    for lc in &merged {
        body.push('\n');
        body.push_str(&emit_library_class(lc)?);
    }
    let mut out = String::from(HEADER);
    let names = model_names(models);
    if !names.is_empty() {
        out.push_str(&format!("\nfrom app.v2.models import {}\n", names.join(", ")));
    }
    if body.contains("RouteHelpers.") {
        out.push_str("from app.v2.route_helpers import RouteHelpers\n");
    }
    if has_importmap && body.contains("Importmap.") {
        out.push_str("from app.v2.importmap import Importmap\n");
    }
    if body.contains("ViewHelpers.") {
        // The framework-namespace collapse renders
        // `ActionView::ViewHelpers` as bare `ViewHelpers`.
        out.push_str("from app.v2.view_helpers import ViewHelpers\n");
    }
    if body.contains("Inflector.") {
        // The inflector unit is Mode::Module (bare functions) — alias
        // the module so `Inflector.pluralize(...)` resolves.
        out.push_str("from app import inflector as Inflector\n");
    }
    if body.contains("JsonBuilder.") {
        // Same Mode::Module aliasing for the jbuilder views' encoder.
        out.push_str("from app import json_builder as JsonBuilder\n");
    }
    out.push_str(&body);
    // Namespace facade: controller/model bodies call `Views.<Resource>.<fn>`.
    out.push_str("\n\nclass Views:\n");
    for name in &order {
        out.push_str(&format!("    {name} = {name}\n"));
    }
    Ok(out)
}

fn emit_controllers(
    controllers: &[LibraryClass],
    models: &[LibraryClass],
) -> Result<String, String> {
    let mut body = String::new();
    for lc in parent_first(controllers) {
        body.push('\n');
        body.push_str(&emit_library_class(lc)?);
    }
    let mut out = String::from(HEADER);
    // ActionController's transpiled base — ApplicationController
    // subclasses its `Base`. Explicit imports throughout: a star
    // import of models would shadow this `Base` with ActiveRecord's.
    // `params as Params`: the strong-param classes' from_raw bodies
    // narrow through the hand-written ParamValue primitives
    // (`Params.sub` / `Params.str_` — see runtime/python/params.py).
    out.push_str("\nfrom app.action_controller_base import Base\n");
    out.push_str("from app import params as Params\n");
    let names = model_names(models);
    if !names.is_empty() {
        out.push_str(&format!("from app.v2.models import {}\n", names.join(", ")));
    }
    out.push_str("from app.v2.views import Views\n");
    if body.contains("RouteHelpers.") {
        out.push_str("from app.v2.route_helpers import RouteHelpers\n");
    }
    if body.contains("Db.") {
        out.push_str("from app.db import Db\n");
    }
    out.push_str(&missing_parent_aliases(controllers));
    out.push_str(&body);
    Ok(out)
}

/// `app/v2/fixtures.py` — one `<Plural>Fixtures` class per fixture
/// set plus `load_all()`, the per-test reset hook the
/// `app.test_support.TestCase` base calls from `setUp`.
fn emit_fixtures(
    fixtures: &[LibraryClass],
    models: &[LibraryClass],
) -> Result<String, String> {
    let mut body = String::new();
    for lc in fixtures {
        body.push('\n');
        body.push_str(&emit_library_class(lc)?);
    }
    let mut out = String::from(HEADER);
    let names = model_names(models);
    if !names.is_empty() {
        out.push_str(&format!("\nfrom app.v2.models import {}\n", names.join(", ")));
    }
    if body.contains("Db.") {
        out.push_str("from app.db import Db\n");
    }
    out.push_str(&body);
    out.push_str("\n\ndef load_all() -> None:\n");
    for lc in fixtures {
        out.push_str(&format!(
            "    {}._fixtures_load_bang()\n",
            last_segment(lc.name.0.as_str())
        ));
    }
    Ok(out)
}

/// One emitted unittest file per test-module LC —
/// `tests/test_<snake>.py` so `python -m unittest` discovery finds
/// it. The LC's parent renders as its last segment
/// (`ActiveSupport::TestCase` → `TestCase`,
/// `ActionDispatch::IntegrationTest` → `IntegrationTest`), supplied
/// by the twin base classes in `app.test_support`.
fn emit_test_file(
    lc: &LibraryClass,
    lcs: &OverlayLcs,
) -> Result<(String, String), String> {
    let class_name = last_segment(lc.name.0.as_str());
    let stem = crate::naming::snake_case(
        class_name.strip_suffix("Test").unwrap_or(class_name),
    );
    let body = emit_library_class(lc)?;
    let parent = lc
        .parent
        .as_ref()
        .map(|p| last_segment(p.0.as_str()).to_string())
        .unwrap_or_else(|| "TestCase".to_string());

    let mut out = String::from(HEADER);
    out.push_str(&format!("\nfrom app.test_support import {parent}\n"));
    let names = model_names(&lcs.models);
    if !names.is_empty() {
        out.push_str(&format!("from app.v2.models import {}\n", names.join(", ")));
    }
    let fixture_names: Vec<String> = lcs
        .fixtures
        .iter()
        .map(|f| last_segment(f.name.0.as_str()).to_string())
        .filter(|n| body.contains(n.as_str()))
        .collect();
    if !fixture_names.is_empty() {
        out.push_str(&format!(
            "from app.v2.fixtures import {}\n",
            fixture_names.join(", ")
        ));
    }
    if body.contains("RouteHelpers.") {
        out.push_str("from app.v2.route_helpers import RouteHelpers\n");
    }
    out.push('\n');
    out.push_str(&body);
    Ok((format!("tests/test_{stem}.py"), out))
}

fn emit_module_class(rh: &LibraryClass) -> Result<String, String> {
    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(&emit_library_class(rh)?);
    Ok(out)
}

/// Dispatch glue: registry of routable controllers + the
/// construct-seed-run-read cycle every thin target's server glue
/// performs (see runtime/crystal/server.cr steps 4-5). The caller
/// (http.py Router, once wired) reads response state back off the
/// returned controller: status / body / location / content_type.
fn emit_dispatch(
    controllers: &[LibraryClass],
    route_table: Option<&LibraryClass>,
    has_layout: bool,
) -> String {
    let mut out = String::from(HEADER);
    out.push_str("\nfrom app import http as _http\n");
    out.push_str("from app.v2.controllers import *  # noqa: F401, F403\n");
    out.push_str("from app.v2.view_helpers import ViewHelpers\n");
    if route_table.is_some() {
        out.push_str("from app.router import Router\n");
        out.push_str("from app.v2.routes import RouteTable\n");
    }
    if has_layout {
        out.push_str("from app.v2.views import Views\n");
    }
    out.push('\n');
    out.push_str("CONTROLLERS = {\n");
    for lc in controllers {
        let class_name = last_segment(lc.name.0.as_str());
        // Routable = defines process_action. Skips the strong-param
        // classes and the abstract ApplicationController.
        let routable = class_name != "ApplicationController"
            && lc.methods.iter().any(|m| m.name.as_str() == "process_action");
        if !routable {
            continue;
        }
        let resource = crate::naming::snake_case(
            class_name.strip_suffix("Controller").unwrap_or(class_name),
        );
        out.push_str(&format!("    \"{resource}\": {class_name},\n"));
    }
    out.push_str("}\n\n");
    out.push_str(
        "def dispatch(controller: str, action: str, *, params=None, session=None,\n\
         \x20            flash=None, request_method: str = \"GET\",\n\
         \x20            request_path: str = \"/\", request_format: str = \"html\"):\n\
         \x20   \"\"\"Construct the controller, seed request state, run the action,\n\
         \x20   and return the controller with response state populated\n\
         \x20   (status / body / location / content_type). Omitted kwargs keep\n\
         \x20   the Base constructor's fresh defaults.\"\"\"\n\
         \x20   c = CONTROLLERS[controller]()\n\
         \x20   if params is not None:\n\
         \x20       c.params = params\n\
         \x20   if session is not None:\n\
         \x20       c.session = session\n\
         \x20   if flash is not None:\n\
         \x20       c.flash = flash\n\
         \x20   c.request_method = request_method\n\
         \x20   c.request_path = request_path\n\
         \x20   c.request_format = request_format\n\
         \x20   # Per-request view-slot reset — the same call every\n\
         \x20   # target's server glue makes before running the action\n\
         \x20   # (crystal server.cr, go slots.go, ts server.ts). Also\n\
         \x20   # what first-creates the module-singleton slots store.\n\
         \x20   ViewHelpers.reset_slots_bang()\n\
         \x20   c.process_action(action)\n\
         \x20   return c\n",
    );
    let Some(rt) = route_table else { return out };

    // `handle` — the full request cycle: transpiled Router.match over
    // the RouteTable, dispatch, layout wrap, ActionResponse
    // translation. The server glue and TestClient both route through
    // it (see runtime/python/server.py / test_support.py).
    let has_root = rt.methods.iter().any(|m| m.name.as_str() == "root");
    let table_expr = if has_root {
        "[RouteTable.root()] + RouteTable.table()"
    } else {
        "RouteTable.table()"
    };
    out.push_str("\n\n_TABLE = None\n\n");
    out.push_str(&format!(
        "def _route_table():\n    global _TABLE\n    if _TABLE is None:\n        _TABLE = {table_expr}\n    return _TABLE\n"
    ));
    out.push_str(
        "\n\ndef _nest_params(flat):\n\
         \x20   \"\"\"Rails bracket keys nest: {\"article[title]\": v} becomes\n\
         \x20   {\"article\": {\"title\": v}} — the shape the strong-param\n\
         \x20   classes narrow (Params.sub). One level deep, matching the\n\
         \x20   form encoder; other keys pass through flat.\"\"\"\n\
         \x20   out = {}\n\
         \x20   for k, v in flat.items():\n\
         \x20       if \"[\" in k and k.endswith(\"]\"):\n\
         \x20           outer, inner = k.split(\"[\", 1)\n\
         \x20           sub = out.setdefault(outer, {})\n\
         \x20           if isinstance(sub, dict):\n\
         \x20               sub[inner[:-1]] = v\n\
         \x20           continue\n\
         \x20       out[k] = v\n\
         \x20   return out\n",
    );
    out.push_str(
        "\n\ndef handle(method: str, path: str, params=None, flash=None):\n\
         \x20   \"\"\"Match + dispatch + translate — the full request cycle.\n\
         \x20   Returns an ActionResponse, or None when no route matches.\n\
         \x20   `flash` is the carried-in Flash (from the cookie); the\n\
         \x20   response's `flash` dict is what the action newly set\n\
         \x20   (Flash.to_persisted's diff), for the server to persist.\"\"\"\n\
         \x20   # Strip a known format extension BEFORE matching, like the\n\
         \x20   # crystal/ts server glue: an unconstrained `:id` segment\n\
         \x20   # would otherwise capture \"1.json\" on the exact-match\n\
         \x20   # pass (Router.match's own ext-retry only runs after an\n\
         \x20   # exact miss).\n\
         \x20   fmt = None\n\
         \x20   last = path.rsplit(\"/\", 1)[-1]\n\
         \x20   if \".\" in last:\n\
         \x20       base, ext = path.rsplit(\".\", 1)\n\
         \x20       if Router.format_extension(ext):\n\
         \x20           path, fmt = base, ext\n\
         \x20   m = Router.match(method, path, _route_table())\n\
         \x20   if m is None:\n\
         \x20       return None\n\
         \x20   p = _nest_params(dict(params or {}))\n\
         \x20   p.update(m.path_params)\n\
         \x20   # Format: a route-declared format wins; else the stripped\n\
         \x20   # `.ext`; else anything the matcher's own ext-retry put in\n\
         \x20   # params[\"format\"].\n\
         \x20   fmt = m.req_format or fmt or p.get(\"format\") or \"html\"\n\
         \x20   c = dispatch(m.controller, m.action, params=p, flash=flash,\n\
         \x20                request_method=method, request_path=path,\n\
         \x20                request_format=fmt)\n\
         \x20   location = c.location or \"\"\n\
         \x20   persisted = c.flash.to_persisted()\n\
         \x20   if fmt != \"html\":\n\
         \x20       ct = c.content_type or \"application/json; charset=utf-8\"\n\
         \x20       return _http.ActionResponse(body=c.body, status=c.status,\n\
         \x20                                   location=location,\n\
         \x20                                   content_type=ct, flash=persisted)\n\
         \x20   body = c.body\n",
    );
    if has_layout {
        out.push_str(
            "\x20   # Layout wrap for rendered HTML; redirects (3xx, empty\n\
             \x20   # body) ship bare. Same sequence as crystal server.cr:\n\
             \x20   # set_yield feeds the layout's `yield` slot.\n\
             \x20   if body and not (300 <= c.status < 400):\n\
             \x20       ViewHelpers.set_yield(body)\n\
             \x20       body = Views.Layouts.application(body, c.flash[\"notice\"],\n\
             \x20                                        c.flash[\"alert\"])\n",
        );
    }
    out.push_str(
        "\x20   return _http.ActionResponse(body=body, status=c.status,\n\
         \x20                               location=location, flash=persisted)\n",
    );
    out
}
