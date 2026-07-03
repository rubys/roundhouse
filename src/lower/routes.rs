//! Target-neutral route flattening.
//!
//! A `RouteTable` stores routes in their source shape —
//! `resources :articles do ... end` blocks, `root "home#index"`
//! shorthand, and explicit `get "/path", to: "c#a"` entries.
//! Every pass-2 emitter needs the expanded, concrete form: one
//! entry per (method, path, controller, action) with a helper
//! name derived from `as:` or the action, plus the list of
//! path-param names.
//!
//! Lifted from six near-identical per-target walkers
//! (`flatten_<lang>_routes` / `collect_flat_<lang>_routes` /
//! `nest_<lang>_path` / `extract_<lang>_path_params` /
//! `<lang>_resource_as_name`). The IR walk is target-independent
//! — only the downstream rendering differs (Go: `ArticlesPath`,
//! Python: `articles_path`, Rust: `articles_path(i64)`, etc.).

use crate::App;
use crate::dialect::{HttpMethod, RouteSpec};
use crate::ident::{ClassId, Symbol};
use crate::naming;

/// One flattened concrete route. `controller` + `action` identify
/// the handler; `as_name` is the route-helper prefix (`"article"`
/// → `article_path`, `"edit_article"` → `edit_article_path`).
/// `path_params` lists param identifiers in declaration order so
/// emitters can build typed helper signatures.
#[derive(Clone, Debug)]
pub struct FlatRoute {
    pub method: HttpMethod,
    pub path: String,
    pub controller: ClassId,
    pub action: Symbol,
    pub as_name: String,
    pub path_params: Vec<String>,
    /// Does this route have a REAL helper name — explicit `as:`,
    /// resources-derived, root, or auto-derived from a fully-static
    /// path? Unnamed dynamic routes carry a legacy action-name
    /// fallback in `as_name` for consumers that key on it, but Rails
    /// generates NO helper for them — the route-helper generator
    /// skips `named: false` entries (an action-name fallback like
    /// `comments` for `/replies/comments/page/:page` would otherwise
    /// shadow the real `/comments` helper).
    pub named: bool,
}

/// The seven standard Rails scaffold actions a `resources` block
/// expands to, in declaration order. Each tuple is
/// `(action_name, http_method, path_suffix)`. Emitters sharing
/// this list see the same registration order — important because
/// `Router.Match` scans in order and the first match wins (e.g.
/// `/articles/new` must come before `/articles/:id`).
pub fn standard_resource_actions() -> &'static [(&'static str, HttpMethod, &'static str)] {
    use HttpMethod::*;
    &[
        ("index", Get, ""),
        ("new", Get, "/new"),
        ("create", Post, ""),
        ("show", Get, "/:id"),
        ("edit", Get, "/:id/edit"),
        ("update", Patch, "/:id"),
        ("destroy", Delete, "/:id"),
    ]
}

/// Flatten every RouteSpec in `app.routes` into the concrete
/// `FlatRoute` list. Resources expand to 7 entries (minus
/// `only`/`except` filters); Root becomes `GET /`; Explicit
/// passes through with its `as:` name preserved.
pub fn flatten_routes(app: &App) -> Vec<FlatRoute> {
    let mut out = Vec::new();
    for entry in &app.routes.entries {
        collect_flat_routes(entry, &mut out, None);
    }
    out
}

/// Route-helper name for a fully-static path: segments joined with `_`
/// (`/search` → `search`, `/comments/upvoted` → `comments_upvoted`).
/// None when any segment is dynamic (`:id`, `*rest`) — Rails generates
/// no helper for an unnamed dynamic route.
fn static_path_name(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty()
        || segs
            .iter()
            .any(|s| s.starts_with(':') || s.starts_with('*') || s.starts_with('('))
    {
        return None;
    }
    let name = segs.join("_").replace('-', "_").replace('.', "_");
    // The name becomes `def self.<name>_path` — a path like "/404"
    // derives a name no target can declare. Fall back to the action
    // name for those (matching the previous behavior).
    if !name.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false) {
        return None;
    }
    Some(name)
}

fn collect_flat_routes(
    spec: &RouteSpec,
    out: &mut Vec<FlatRoute>,
    scope: Option<(&str, &str)>,
) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, as_name, .. } => {
            let (full_path, mut params) = nest_path(path, scope);
            extract_path_params(&full_path, &mut params);
            // Rails auto-names a plain `get "/search" => "search#index"`
            // route from its fully-static path (`search_path`).
            // Dynamic-segment paths get no auto name in Rails; keep the
            // legacy action-name fallback in `as_name` for consumers
            // that key on it, but mark the route unnamed so the helper
            // generator skips it.
            let (derived_name, named) = match as_name.as_ref() {
                Some(s) => (s.as_str().to_string(), true),
                None => match static_path_name(&full_path) {
                    Some(n) => (n, true),
                    None => (action.as_str().to_string(), false),
                },
            };
            out.push(FlatRoute {
                method: method.clone(),
                path: full_path,
                controller: controller.clone(),
                action: action.clone(),
                as_name: derived_name,
                path_params: params,
                named,
            });
        }
        RouteSpec::Root { target } => {
            let (controller_name, action_name) = target
                .split_once('#')
                .map(|(c, a)| (c.to_string(), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            // `Root` in the IR carries the raw "controller#action"
            // string, not a parsed ClassId. Re-build the
            // `XxxController` class name so the shape matches what
            // Explicit / Resources produce.
            let controller_class = format!(
                "{}Controller",
                naming::camelize(&controller_name)
            );
            out.push(FlatRoute {
                method: HttpMethod::Get,
                path: "/".to_string(),
                controller: ClassId(Symbol::from(controller_class)),
                action: Symbol::from(action_name),
                as_name: "root".to_string(),
                path_params: vec![],
                named: true,
            });
        }
        RouteSpec::Resources { name, only, except, nested } => {
            let resource_path = format!("/{name}");
            let controller_class = ClassId(Symbol::from(format!(
                "{}Controller",
                naming::camelize(name.as_str())
            )));
            let singular_low =
                naming::singularize_camelize(name.as_str()).to_lowercase();

            for (action, method, suffix) in standard_resource_actions() {
                let action_name: &str = action;
                let suffix: &str = suffix;
                if !only.is_empty()
                    && !only.iter().any(|s| s.as_str() == action_name)
                {
                    continue;
                }
                if except.iter().any(|s| s.as_str() == action_name) {
                    continue;
                }
                let path = format!("{resource_path}{suffix}");
                let (full_path, mut params) = nest_path(&path, scope);
                if suffix.contains(":id") && !params.iter().any(|p| p == "id") {
                    params.push("id".to_string());
                }
                let as_name =
                    resource_as_name(action_name, &singular_low, name.as_str(), scope);
                out.push(FlatRoute {
                    method: method.clone(),
                    path: full_path,
                    controller: controller_class.clone(),
                    action: Symbol::from(action_name),
                    as_name,
                    path_params: params,
                    named: true,
                });
            }
            for child in nested {
                collect_flat_routes(child, out, Some((&singular_low, name.as_str())));
            }
        }
    }
}

/// Prepend a scope's `/<parent>/:parent_id` prefix to a child path.
/// Returns the full path and the list of path-param names in
/// declaration order (parent first).
fn nest_path(path: &str, scope: Option<(&str, &str)>) -> (String, Vec<String>) {
    match scope {
        Some((parent, parent_plural)) => {
            let full = format!("/{parent_plural}/:{parent}_id{path}");
            let params = vec![format!("{parent}_id")];
            (full, params)
        }
        None => (path.to_string(), vec![]),
    }
}

/// Walk a Rails-shape path (`/posts/:id/edit`,
/// `/articles/:article_id/comments`) and append any `:param`
/// segment names not already in `params`.
fn extract_path_params(path: &str, params: &mut Vec<String>) {
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' {
            let mut ident = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    ident.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if !ident.is_empty() && !params.iter().any(|p| p == &ident) {
                params.push(ident);
            }
        }
    }
}

/// Route-helper base name for a standard Rails action. The
/// emitter then appends `_path` / `_url` / `Path` / `_url` per
/// target convention.
///
/// - `index`/`create` → plural (`articles`, `article_comments`)
/// - `new` → `new_<singular>` (`new_article`)
/// - `edit` → `edit_<singular>` (`edit_article`)
/// - `show`/`update`/`destroy` → singular (`article`)
fn resource_as_name(
    action: &str,
    singular_low: &str,
    plural: &str,
    scope: Option<(&str, &str)>,
) -> String {
    let parent_prefix = scope
        .map(|(p, _)| format!("{p}_"))
        .unwrap_or_default();
    match action {
        "index" | "create" => format!("{parent_prefix}{plural}"),
        "new" => format!("new_{parent_prefix}{singular_low}"),
        "edit" => format!("edit_{parent_prefix}{singular_low}"),
        _ => format!("{parent_prefix}{singular_low}"),
    }
}
