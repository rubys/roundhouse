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
use crate::dialect::{HttpMethod, ResourceScope, RouteSpec};
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
    /// Route-forced response format — the `:format => "rss"` option on
    /// an explicit route (`get "/rss" => "home#index", :format =>
    /// "rss"`). Dispatch seeds the controller's `request_format` from
    /// it so the `respond_to`-flattened branch picks the right view.
    /// None (the overwhelmingly common case) leaves format inference
    /// to the request path.
    pub format: Option<Symbol>,
    /// Count of LEADING `path_params` that are REQUIRED; the rest come
    /// from trailing Rails optional groups (`get "/top(/:length(/page/
    /// :page))"`) and get `nil`-defaulted helper params whose path
    /// segments are appended only when supplied. Equals `path_params.
    /// len()` for the common all-required route.
    pub required_params: usize,
    /// Path params constrained to digit-only segments — Roda's
    /// `Integer` matcher (`r.on Integer`) and Rails digit-class
    /// `constraints:` (`/\d+/`, `/[0-9]+/`). The runtime router
    /// rejects the route when the captured segment isn't all digits,
    /// so `/articles/12abc` falls through to 404 instead of binding
    /// `id = "12abc"` (and, post-`to_i`, serving article 12).
    /// Constraint regexes beyond the digit class aren't modeled — the
    /// runtime router is deliberately regex-free.
    pub int_params: Vec<String>,
}

/// Is this constraint regex a plain digit class (`\d+` / `[0-9]+`,
/// optionally `\A…\z` / `^…$` anchored)? Those are the only
/// constraints the regex-free runtime router can enforce; anything
/// else keeps the pre-existing dropped-at-lowering behavior.
fn digit_class_regex(rx: &str) -> bool {
    let rx = rx
        .strip_prefix("\\A")
        .or_else(|| rx.strip_prefix('^'))
        .unwrap_or(rx);
    let rx = rx
        .strip_suffix("\\z")
        .or_else(|| rx.strip_suffix('$'))
        .unwrap_or(rx);
    rx == "\\d+" || rx == "[0-9]+"
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
/// passes through with its `as:` name preserved; Scope entries
/// compose their path/module/helper facets onto everything nested.
pub fn flatten_routes(app: &App) -> Vec<FlatRoute> {
    let mut out = Vec::new();
    let ctx = Ctx::default();
    for entry in &app.routes.entries {
        collect_flat_routes(entry, &mut out, &ctx);
    }
    out
}

/// Accumulated flattening context: the namespace/scope facets from
/// enclosing [`RouteSpec::Scope`] entries plus the enclosing
/// `resources` (singular, plural) for member nesting.
#[derive(Clone, Default)]
struct Ctx {
    /// URL prefix, e.g. `/admin` (empty at top level).
    ns_path: String,
    /// Controller-class prefix, camelized per segment: `Admin::`.
    module_prefix: String,
    /// Helper-name prefix: `admin_`.
    name_prefix: String,
    /// Enclosing `resources` (singular, plural), reset by Scope.
    parent: Option<(String, String)>,
}

impl Ctx {
    fn parent_pair(&self) -> Option<(&str, &str)> {
        self.parent.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))
    }
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

/// The six actions a *singular* `resource :name` expands to — no
/// `index`, no `:id` segment (`GET /profile`, `PATCH /profile`, …).
fn singular_resource_actions() -> &'static [(&'static str, HttpMethod, &'static str)] {
    use HttpMethod::*;
    &[
        ("new", Get, "/new"),
        ("create", Post, ""),
        ("show", Get, ""),
        ("edit", Get, "/edit"),
        ("update", Patch, ""),
        ("destroy", Delete, ""),
    ]
}

fn collect_flat_routes(spec: &RouteSpec, out: &mut Vec<FlatRoute>, ctx: &Ctx) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, as_name, scope, constraints } => {
            // `:format => "rss"` rides the constraints map at ingest
            // (it shapes the request, not the routing triple); surface
            // it as the route's forced response format.
            let forced_format = constraints
                .get(&Symbol::from("format"))
                .map(|f| Symbol::from(f.as_str()));
            let (nested, base_params) = nest_path(path, ctx.parent_pair(), *scope);
            let full_path = prefix_path(&ctx.ns_path, &nested);
            // Rails optional `(…)` segments (`get "/s/:id/(:title)"`) match
            // whether or not the segment is present; expand them into
            // concrete routes (`/s/:id/:title` and `/s/:id`) so the
            // segment-count router matches both. Paths with no optional
            // group yield a single unchanged entry.
            let variants = expand_optional_paths(&full_path);
            // Rails auto-names a plain `get "/search" => "search#index"`
            // route from its fully-static path (`search_path` —
            // namespace segments included: `/api/oembed` →
            // `api_oembed`). Dynamic-segment paths get no auto name in
            // Rails; keep the legacy action-name fallback in `as_name`
            // for consumers that key on it, but mark the route unnamed
            // so the helper generator skips it. Name derives from the
            // canonical (first, longest) variant.
            let (derived_name, named) = match as_name.as_ref() {
                Some(s) => (format!("{}{}", ctx.name_prefix, s.as_str()), true),
                None => match static_path_name(&variants[0]) {
                    Some(n) => (n, true),
                    // A static child path nested under a resource scope
                    // (`get "suggest"` inside `resources :stories`) is
                    // auto-named `<singular-parent>_<child>` by Rails
                    // (`story_suggest_path(story_id)`) even though the
                    // full path carries the dynamic `:story_id`.
                    None => match ctx.parent_pair().and_then(|(parent, _)| {
                        static_path_name(path)
                            .map(|child| format!("{}{parent}_{child}", ctx.name_prefix))
                    }) {
                        Some(n) => (n, true),
                        None => (action.as_str().to_string(), false),
                    },
                },
            };
            // The SHORTEST variant (last — variants are longest-first)
            // fixes how many leading params are required; the extras the
            // longer variants add are the trailing optional-group params.
            // The canonical (named) helper carries that required count so
            // its optional params get `nil` defaults.
            //
            // The optional-path helper body (build_optional_path_expr)
            // assumes the optional groups are TRAILING — each shorter
            // variant is a segment-prefix of the longer. A MID-path group
            // (`/foo(/:bar)/baz`) breaks that: the shortest ("/foo/baz")
            // isn't a prefix of the longest ("/foo/:bar/baz"), and the
            // conditional-append body would fold the always-present "/baz"
            // into `:bar`'s optional chunk. No lobsters route has one; when
            // the assumption doesn't hold, fall back to all-required (a
            // valid, if over-constrained, helper) rather than emit a
            // mangled path.
            let trailing_optionals = {
                let short: Vec<&str> = variants.last().unwrap().split('/').collect();
                let long: Vec<&str> = variants[0].split('/').collect();
                short.len() <= long.len()
                    && short.iter().zip(long.iter()).all(|(a, b)| a == b)
            };
            let required_count = {
                let mut p = base_params.clone();
                let shortest = if trailing_optionals { variants.last() } else { variants.first() };
                extract_path_params(shortest.unwrap(), &mut p);
                p.len()
            };
            // Only the canonical variant carries the helper name; the
            // shorter alternates would otherwise register a duplicate
            // helper for the same controller#action.
            // Digit-class constraints (`\d+` — Roda `Integer` matcher,
            // Rails `constraints: { id: /\d+/ }`) become enforceable
            // router metadata; anything fancier stays dropped.
            let digit_params: Vec<String> = constraints
                .iter()
                .filter(|(name, rx)| {
                    name.as_str() != "format" && digit_class_regex(rx)
                })
                .map(|(name, _)| name.as_str().to_string())
                .collect();
            for (i, vpath) in variants.into_iter().enumerate() {
                let mut params = base_params.clone();
                extract_path_params(&vpath, &mut params);
                let required_params = if i == 0 { required_count } else { params.len() };
                // A shorter optional-group variant may not carry every
                // constrained param — keep only the ones it captures.
                let int_params: Vec<String> = digit_params
                    .iter()
                    .filter(|n| params.contains(n))
                    .cloned()
                    .collect();
                out.push(FlatRoute {
                    method: method.clone(),
                    path: vpath,
                    controller: qualify_controller(&ctx.module_prefix, controller),
                    action: action.clone(),
                    as_name: derived_name.clone(),
                    path_params: params,
                    named: named && i == 0,
                    format: forced_format.clone(),
                    required_params,
                    int_params,
                });
            }
        }
        RouteSpec::Root { target } => {
            let (controller_name, action_name) = target
                .split_once('#')
                .map(|(c, a)| (c.to_string(), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            // `Root` in the IR carries the raw "controller#action"
            // string, not a parsed ClassId. Re-build the
            // `XxxController` class name so the shape matches what
            // Explicit / Resources produce. Inside a namespace, `root`
            // maps the scope's own prefix (`GET /admin` →
            // `admin_root`).
            let controller_class = format!(
                "{}{}Controller",
                ctx.module_prefix,
                naming::camelize(&controller_name)
            );
            let path =
                if ctx.ns_path.is_empty() { "/".to_string() } else { ctx.ns_path.clone() };
            out.push(FlatRoute {
                method: HttpMethod::Get,
                path,
                controller: ClassId(Symbol::from(controller_class)),
                action: Symbol::from(action_name),
                as_name: format!("{}root", ctx.name_prefix),
                path_params: vec![],
                named: true,
                format: None,
                required_params: 0,
                int_params: vec![],
            });
        }
        RouteSpec::Resources { name, only, except, nested, singular } => {
            let resource_path = format!("/{name}");
            // `resource :profile` still routes to the *plural*
            // controller (`ProfilesController`), per Rails.
            let controller_stem = if *singular {
                naming::camelize(&naming::pluralize_snake(name.as_str()))
            } else {
                naming::camelize(name.as_str())
            };
            let controller_class = ClassId(Symbol::from(format!(
                "{}{}Controller",
                ctx.module_prefix, controller_stem
            )));
            // Snake-preserving singular (`domain_allows` →
            // `domain_allow`): camelize+lowercase would collapse the
            // underscores out of helper names and `:parent_id` params.
            let singular_low = if *singular {
                name.as_str().to_string()
            } else {
                naming::singularize(name.as_str())
            };
            let actions = if *singular {
                singular_resource_actions()
            } else {
                standard_resource_actions()
            };

            for (action, method, suffix) in actions {
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
                let (nested_path, mut params) =
                    nest_path(&path, ctx.parent_pair(), ResourceScope::Nested);
                let full_path = prefix_path(&ctx.ns_path, &nested_path);
                if suffix.contains(":id") && !params.iter().any(|p| p == "id") {
                    params.push("id".to_string());
                }
                let as_name = resource_as_name(
                    action_name,
                    &singular_low,
                    name.as_str(),
                    ctx.parent_pair(),
                    &ctx.name_prefix,
                );
                out.push(FlatRoute {
                    method: method.clone(),
                    path: full_path,
                    controller: controller_class.clone(),
                    action: Symbol::from(action_name),
                    as_name,
                    required_params: params.len(),
                    path_params: params,
                    named: true,
                    format: None,
                    int_params: vec![],
                });
            }
            let child_ctx = Ctx {
                parent: Some((singular_low.clone(), name.as_str().to_string())),
                ..ctx.clone()
            };
            for child in nested {
                collect_flat_routes(child, out, &child_ctx);
            }
        }
        RouteSpec::Scope { path, module, as_prefix, entries } => {
            let mut child = ctx.clone();
            // Scope boundaries reset the resources-nesting context —
            // mirrors the ingester's controller-inference reset.
            child.parent = None;
            if let Some(p) = path {
                child.ns_path = prefix_path(&ctx.ns_path, &format!("/{}", p.trim_matches('/')));
            }
            if let Some(m) = module {
                for seg in m.split('/').filter(|s| !s.is_empty()) {
                    child.module_prefix.push_str(&naming::camelize(seg));
                    child.module_prefix.push_str("::");
                }
            }
            if let Some(a) = as_prefix {
                child.name_prefix.push_str(a);
                child.name_prefix.push('_');
            }
            for entry in entries {
                collect_flat_routes(entry, out, &child);
            }
        }
    }
}

/// Prepend the accumulated namespace path. Both sides are `/`-rooted
/// segments; guard the bare-`/` and missing-slash edges (`root` inside
/// a namespace, `get "health"` with no leading slash).
fn prefix_path(ns: &str, path: &str) -> String {
    if ns.is_empty() {
        if path.starts_with('/') || path.is_empty() {
            path.to_string()
        } else {
            format!("/{path}")
        }
    } else if path == "/" || path.is_empty() {
        ns.to_string()
    } else if path.starts_with('/') {
        format!("{ns}{path}")
    } else {
        format!("{ns}/{path}")
    }
}

/// `Admin::` + `UsersController` → the module-qualified class the
/// scoped route dispatches to.
fn qualify_controller(module_prefix: &str, controller: &ClassId) -> ClassId {
    if module_prefix.is_empty() {
        controller.clone()
    } else {
        ClassId(Symbol::from(format!("{module_prefix}{}", controller.0.as_str())))
    }
}

/// Prepend a scope's `/<parent>/:parent_id` prefix to a child path.
/// Returns the full path and the list of path-param names in
/// declaration order (parent first).
fn nest_path(
    path: &str,
    scope: Option<(&str, &str)>,
    rscope: ResourceScope,
) -> (String, Vec<String>) {
    let Some((parent, parent_plural)) = scope else {
        return (path.to_string(), vec![]);
    };
    match rscope {
        // `member do get "reply" end` → `/comments/:id/reply` (`:id`, the
        // record's own key — what a controller's `find` reads as
        // `params[:id]`). An already-structured path inside `member`
        // (`get "/comments/:id" => …`, a leading-slash absolute route) is
        // used verbatim, matching Rails' escape from the nesting.
        ResourceScope::Member => {
            if is_bare_child_segment(path) {
                (format!("/{parent_plural}/:id{path}"), vec!["id".to_string()])
            } else {
                (path.to_string(), vec![])
            }
        }
        // `collection do get "search" end` → `/photos/search` (no id).
        ResourceScope::Collection => {
            if is_bare_child_segment(path) {
                (format!("/{parent_plural}{path}"), vec![])
            } else {
                (path.to_string(), vec![])
            }
        }
        // Bare verb declared directly in the block, or a nested resource's
        // own actions: Rails nests under the parent's `/:<singular>_id`.
        ResourceScope::Nested => {
            let full = format!("/{parent_plural}/:{parent}_id{path}");
            (full, vec![format!("{parent}_id")])
        }
    }
}

/// A single bare path segment like `/reply` (from a `get "reply"`
/// shortcut) — no interior `/` and no `:param`. Such a member/collection
/// child is nested under the parent; a structured path (`/comments/:id`)
/// is an absolute override used as-is.
fn is_bare_child_segment(path: &str) -> bool {
    let trimmed = path.trim_matches('/');
    !trimmed.is_empty() && !trimmed.contains('/') && !trimmed.contains(':')
}

/// Expand a Rails path with optional `(…)` groups into the concrete
/// paths it can match. `/s/:id/(:title)` → `["/s/:id/:title", "/s/:id"]`
/// (canonical/longest first). An inline optional format suffix
/// (`/domains/:id(.:format)`) can't be matched by the slash-segment
/// router, so only the base path is kept. Paths with no group return
/// themselves unchanged.
fn expand_optional_paths(path: &str) -> Vec<String> {
    let Some(open) = path.find('(') else {
        return vec![path.to_string()];
    };
    // Depth-matched close: Rails optional groups nest
    // (`/top(/:length(/page/:page))`), and pairing with the FIRST `)`
    // would leave the outer close as a stray literal in the
    // without-branch (`"/top)"`).
    let mut depth = 0usize;
    let mut close_found = None;
    for (i, c) in path.char_indices().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close_found = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close_found else {
        // Unbalanced parens — strip the stray `(` defensively.
        return vec![path.replace('(', "")];
    };
    let before = &path[..open];
    let inside = &path[open + 1..close];
    let after = &path[close + 1..];
    let without = {
        let joined = format!("{before}{after}").replace("//", "/");
        let trimmed = joined.trim_end_matches('/');
        if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() }
    };
    let mut out = Vec::new();
    // Inline optional format suffix (`(.:format)`) — a dotted segment the
    // slash-splitting router can't capture; drop it, keep only the base.
    if !inside.starts_with('.') {
        out.extend(expand_optional_paths(&format!("{before}{inside}{after}")));
    }
    out.extend(expand_optional_paths(&without));
    out
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
///
/// `ns_prefix` is the accumulated namespace helper prefix (`admin_`);
/// Rails keeps the verb first (`new_admin_domain_allow`), so it slots
/// after `new_`/`edit_` alongside the parent prefix.
fn resource_as_name(
    action: &str,
    singular_low: &str,
    plural: &str,
    scope: Option<(&str, &str)>,
    ns_prefix: &str,
) -> String {
    let parent_prefix = scope
        .map(|(p, _)| format!("{p}_"))
        .unwrap_or_default();
    match action {
        "index" | "create" => format!("{ns_prefix}{parent_prefix}{plural}"),
        "new" => format!("new_{ns_prefix}{parent_prefix}{singular_low}"),
        "edit" => format!("edit_{ns_prefix}{parent_prefix}{singular_low}"),
        _ => format!("{ns_prefix}{parent_prefix}{singular_low}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_route_nests_under_id() {
        // `member do get "reply" end` in `resources :comments` — the
        // record's own key, so `find_comment` reads `params[:id]`.
        let (path, params) =
            nest_path("/reply", Some(("comment", "comments")), ResourceScope::Member);
        assert_eq!(path, "/comments/:id/reply");
        assert_eq!(params, vec!["id".to_string()]);
    }

    #[test]
    fn member_route_absolute_path_used_verbatim() {
        // `get "/comments/:id" => …` inside a member block escapes nesting.
        let (path, params) = nest_path(
            "/comments/:id",
            Some(("comment", "comments")),
            ResourceScope::Member,
        );
        assert_eq!(path, "/comments/:id");
        assert!(params.is_empty());
    }

    #[test]
    fn collection_route_has_no_id_segment() {
        let (path, params) =
            nest_path("/search", Some(("photo", "photos")), ResourceScope::Collection);
        assert_eq!(path, "/photos/search");
        assert!(params.is_empty());
    }

    #[test]
    fn bare_verb_in_resources_keeps_parent_id() {
        // `post "upvote"` directly in `resources :stories` → `:story_id`.
        let (path, params) =
            nest_path("/upvote", Some(("story", "stories")), ResourceScope::Nested);
        assert_eq!(path, "/stories/:story_id/upvote");
        assert_eq!(params, vec!["story_id".to_string()]);
    }

    #[test]
    fn top_level_route_is_unnested() {
        let (path, params) = nest_path("/login", None, ResourceScope::Nested);
        assert_eq!(path, "/login");
        assert!(params.is_empty());
    }

    #[test]
    fn optional_trailing_segment_expands_both_ways() {
        assert_eq!(
            expand_optional_paths("/s/:id/(:title)"),
            vec!["/s/:id/:title".to_string(), "/s/:id".to_string()]
        );
    }

    #[test]
    fn inline_optional_format_is_dropped() {
        assert_eq!(
            expand_optional_paths("/domains/:id(.:format)"),
            vec!["/domains/:id".to_string()]
        );
    }

    #[test]
    fn path_without_optional_group_is_unchanged() {
        assert_eq!(expand_optional_paths("/s/:id"), vec!["/s/:id".to_string()]);
    }
}
