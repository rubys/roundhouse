//! Python universal-IR overlay — the strangler replacing the
//! per-artifact emit (models/views/controllers derived from
//! Rails-shape IR, controllers via `lower::CtrlWalker`).
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
//! NOT yet wired into `emit()` — exercised by
//! `tests/python_controller_library_emit.rs` (emit + py_compile
//! inventory) until the http.py dispatch glue lands and the
//! switchover flips. See docs/python-overlay-plan.md.

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
    let views =
        crate::lower::lower_views_to_library_classes(&app.views, app, model_extras);
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    let route_helpers = if route_helper_funcs.is_empty() {
        None
    } else {
        Some(crate::lower::module_funcs_to_library_class(
            "RouteHelpers",
            &route_helper_funcs,
        ))
    };
    OverlayLcs { controllers, models, views, route_helpers }
}

/// Emit the overlay file set. Fails loudly on the first class the
/// walker can't render — the inventory test reports per-class.
pub fn emit_overlay_files(app: &App) -> Result<Vec<EmittedFile>, String> {
    let lcs = lower_overlay(app);
    let mut files = Vec::new();
    files.push(file("app/v2/__init__.py", String::new()));
    files.push(file("app/v2/models.py", emit_models(&lcs.models)?));
    files.push(file("app/v2/views.py", emit_views(&lcs.views, &lcs.models)?));
    files.push(file(
        "app/v2/controllers.py",
        emit_controllers(&lcs.controllers, &lcs.models)?,
    ));
    if let Some(rh) = &lcs.route_helpers {
        files.push(file("app/v2/route_helpers.py", emit_route_helpers(rh)?));
    }
    files.push(file("app/v2/dispatch.py", emit_dispatch(&lcs.controllers)));
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
    ] {
        if body.contains(needle) {
            out.push_str(import);
        }
    }
    out.push_str(&body);
    // Bottom import breaks the models↔views cycle (see module doc).
    if body.contains("Views.") {
        out.push_str("\nfrom app.v2.views import Views  # noqa: E402 — cycle-breaking bottom import\n");
    }
    Ok(out)
}

fn emit_views(views: &[LibraryClass], models: &[LibraryClass]) -> Result<String, String> {
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
    // TODO(overlay wiring): `ActionView.ViewHelpers.*` call sites need
    // the view-helper runtime — either graduate the transpiled
    // `action_view/view_helpers` unit into LIVE_FRAMEWORK or shim the
    // hand-written app/view_helpers.py under these names.
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
    out.push_str("\nfrom app.action_controller_base import Base\n");
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
    out.push_str(&body);
    Ok(out)
}

fn emit_route_helpers(rh: &LibraryClass) -> Result<String, String> {
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
fn emit_dispatch(controllers: &[LibraryClass]) -> String {
    let mut out = String::from(HEADER);
    out.push_str("\nfrom app.v2.controllers import *  # noqa: F401, F403\n\n");
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
        "def dispatch(controller: str, action: str, *, params, session, flash,\n\
         \x20            request_method: str, request_path: str, request_format: str):\n\
         \x20   \"\"\"Construct the controller, seed request state, run the action,\n\
         \x20   and return the controller with response state populated.\"\"\"\n\
         \x20   c = CONTROLLERS[controller]()\n\
         \x20   c.params = params\n\
         \x20   c.session = session\n\
         \x20   c.flash = flash\n\
         \x20   c.request_method = request_method\n\
         \x20   c.request_path = request_path\n\
         \x20   c.request_format = request_format\n\
         \x20   c.process_action(action)\n\
         \x20   return c\n",
    );
    out
}
