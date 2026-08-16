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
    /// Segment values Rails fills in when the caller omits them —
    /// `scope defaults: { user_id: "me" }`. The generated helper takes
    /// such a param OPTIONALLY, defaulted to this value, which is why
    /// this rides the flat route rather than staying a request-shaping
    /// detail: campfire calls `user_profile_url` with no argument at
    /// all, and the helper's own signature is what has to allow it.
    pub param_defaults: Vec<(String, String)>,
    pub int_params: Vec<String>,
    /// Non-digit `constraints:` regexes, as `(param, regex_source)` for
    /// the params this route captures. The regex-free runtime router
    /// ignores these (digit-class ones ride `int_params`), but the
    /// `--target roda` converter needs them: two routes that share a
    /// path+verb and differ ONLY by such a constraint (Lobsters'
    /// `/t/:tag` single-vs-multi tag) would otherwise collapse to
    /// duplicate branches, the second unreachable. The converter emits
    /// them as an `if <regex>.match?(var)` guard. `format` is excluded
    /// (it rides `format`); digit-class regexes are excluded (they ride
    /// `int_params` / the `Integer` matcher).
    pub constraints: Vec<(String, String)>,
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
    /// Segment defaults from enclosing `scope defaults: { … }` entries,
    /// innermost last so a nested scope can override an outer one.
    param_defaults: Vec<(String, String)>,
    /// Enclosing `resources`/`resource` blocks, OUTERMOST FIRST.
    ///
    /// A stack rather than one pair because Rails accumulates every
    /// level, in both the path and the helper name: `resource :account`
    /// wrapping `resources :bots` wrapping `resource :key` is
    /// `account_bot_key_path` at `/account/bots/:bot_id/key`. Holding
    /// only the innermost lost `account` from both.
    parents: Vec<Nesting>,
}

/// One enclosing resource.
#[derive(Clone, Debug, PartialEq)]
struct Nesting {
    /// Helper-name segment (`user`, `account`); accumulates into the
    /// prefix in declaration order.
    singular: String,
    /// Path segment, which is the name as written (`users`, `account`).
    plural: String,
    /// Whether this level contributes a `/:<singular>_id` segment.
    ///
    /// FALSE for a singular `resource :account`: Rails routes its
    /// children at `/account/…` with no `:account_id`, because there is
    /// only ever one. Getting this wrong put a phantom id in the middle
    /// of every path under a singular parent (`/account/:account_id/logo`).
    has_id: bool,
}

impl Ctx {
    /// The innermost enclosing resource — what controller inference and
    /// the bare-verb shortcuts key off.
    fn parent_pair(&self) -> Option<(&str, &str)> {
        self.parents
            .last()
            .map(|n| (n.singular.as_str(), n.plural.as_str()))
    }

    /// `account_bot_` — every enclosing level's singular, in order.
    fn parent_name_prefix(&self) -> String {
        self.parents
            .iter()
            .map(|n| format!("{}_", n.singular))
            .collect()
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
            let (nested, base_params) = nest_path(path, &ctx.parents, *scope);
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
            // A member/collection child takes its name from the SCOPE,
            // and that rule has to be consulted BEFORE the generic
            // static-path one below: a collection path is fully static
            // (`/comments/requested`), so the static rule would claim it
            // and yield `comments_requested` where Rails says
            // `requested_comments`. Member paths carry `:id` and so fall
            // through the static rule anyway, but both are decided here
            // to keep the scope rule in one place.
            let scoped_name = match scope {
                ResourceScope::Member | ResourceScope::Collection => {
                    ctx.parent_pair().and_then(|(parent, parent_plural)| {
                        static_path_name(path).map(|child| match scope {
                            ResourceScope::Member => {
                                format!("{}{child}_{parent}", ctx.name_prefix)
                            }
                            _ => format!("{}{child}_{parent_plural}", ctx.name_prefix),
                        })
                    })
                }
                // A bare verb in the resources block is a nested route;
                // it keeps the parent-first order, decided below.
                ResourceScope::Nested => None,
            };
            let (derived_name, named) = match as_name.as_ref() {
                // Where an explicit `as:` sits relative to the enclosing
                // resource depends on the scope, and the two go OPPOSITE
                // ways. Measured against Rails 8.1 inside
                // `resources :rooms`:
                //
                //   get "@:id", as: :at_message        → room_at_message
                //   get :preview, on: :member, as: :peek → peek_room
                //   get :recent, on: :collection, as: :fresh → fresh_rooms
                //
                // A nested route reads parent-first (it is a thing
                // BELONGING to the room); a member/collection route reads
                // verb-first (it is an ACTION on the room). Prefixing
                // unconditionally — which this did — cost campfire
                // `room_at_message` and `room_bot_messages`.
                Some(s) => {
                    let s = s.as_str();
                    let n = match (scope, ctx.parent_pair()) {
                        (ResourceScope::Member, Some((parent, _))) => {
                            format!("{}{s}_{parent}", ctx.name_prefix)
                        }
                        (ResourceScope::Collection, Some((_, plural))) => {
                            format!("{}{s}_{plural}", ctx.name_prefix)
                        }
                        (ResourceScope::Nested, _) => {
                            format!("{}{}{s}", ctx.name_prefix, ctx.parent_name_prefix())
                        }
                        // No enclosing resource — the name stands alone.
                        _ => format!("{}{s}", ctx.name_prefix),
                    };
                    (n, true)
                }
                None => match scoped_name {
                    Some(n) => (n, true),
                    None => match static_path_name(&variants[0]) {
                        Some(n) => (n, true),
                        // A bare verb declared directly in the `resources`
                        // block is a NESTED route, and Rails names it
                        // `<singular-parent>_<child>` (`story_suggest_path`)
                        // even though the full path carries the dynamic
                        // `:story_id` that kept it out of the static rule
                        // above. Member/collection never reach here — they
                        // were decided by `scoped_name`.
                        None => match ctx.parent_pair().and_then(|(parent, _)| {
                            static_path_name(path)
                                .map(|child| format!("{}{parent}_{child}", ctx.name_prefix))
                        }) {
                            Some(n) => (n, true),
                            None => (action.as_str().to_string(), false),
                        },
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
            // Non-digit constraints the runtime router can't enforce but
            // the roda converter uses to disambiguate same-path routes.
            let other_constraints: Vec<(String, String)> = constraints
                .iter()
                .filter(|(name, rx)| {
                    name.as_str() != "format" && !digit_class_regex(rx)
                })
                .map(|(name, rx)| (name.as_str().to_string(), rx.clone()))
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
                let constraints: Vec<(String, String)> = other_constraints
                    .iter()
                    .filter(|(n, _)| params.contains(n))
                    .cloned()
                    .collect();
                out.push(FlatRoute {
                    method: method.clone(),
                    path: vpath,
                    controller: qualify_controller(&ctx.module_prefix, controller),
                    action: action.clone(),
                    as_name: derived_name.clone(),
                    named: named && i == 0,
                    format: forced_format.clone(),
                    required_params,
                    param_defaults: defaults_for(ctx, &params),
                    path_params: params,
                    int_params,
                    constraints,
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
                param_defaults: vec![],
                named: true,
                format: None,
                required_params: 0,
                int_params: vec![],
                constraints: vec![],
            });
        }
        RouteSpec::Resources { name, only, except, nested, singular, as_name } => {
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
            // `as:` renames the HELPERS only — the path above already
            // came from `name`. Rails still prepends the namespace, so
            // `namespace :mod { resources :mails, as: "mod_mails" }`
            // yields `mod_mod_mails_path` / `mod_mod_mail_path`.
            let (helper_singular, helper_plural) = match as_name {
                Some(a) if *singular => (a.as_str().to_string(), a.as_str().to_string()),
                Some(a) => (naming::singularize(a.as_str()), a.as_str().to_string()),
                None => (singular_low.clone(), name.as_str().to_string()),
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
                    nest_path(&path, &ctx.parents, ResourceScope::Nested);
                let full_path = prefix_path(&ctx.ns_path, &nested_path);
                if suffix.contains(":id") && !params.iter().any(|p| p == "id") {
                    params.push("id".to_string());
                }
                let as_name = resource_as_name(
                    action_name,
                    &helper_singular,
                    &helper_plural,
                    &ctx.parent_name_prefix(),
                    &ctx.name_prefix,
                );
                out.push(FlatRoute {
                    method: method.clone(),
                    path: full_path,
                    controller: controller_class.clone(),
                    action: Symbol::from(action_name),
                    as_name,
                    required_params: params.len(),
                    param_defaults: defaults_for(ctx, &params),
                    path_params: params,
                    named: true,
                    format: None,
                    int_params: vec![],
                    constraints: vec![],
                });
            }
            let child_ctx = Ctx {
                parents: {
                    let mut p = ctx.parents.clone();
                    p.push(Nesting {
                        singular: singular_low.clone(),
                        plural: name.as_str().to_string(),
                        has_id: !*singular,
                    });
                    p
                },
                ..ctx.clone()
            };
            for child in nested {
                collect_flat_routes(child, out, &child_ctx);
            }
        }
        RouteSpec::Scope { path, module, as_prefix, defaults, entries } => {
            let mut child = ctx.clone();
            // Resource nesting SURVIVES a scope/namespace boundary —
            // measured against Rails 8.1, which is the only authority
            // here:
            //
            //   resources :users do
            //     scope module: "users" { resource :avatar }   # /users/:user_id/avatar
            //     namespace :admin     { resources :notes }    # /users/:user_id/admin/notes
            //     scope :extra         { resource :badge }     # /extra/users/:user_id/badge
            //   end
            //
            // all keep the `user_` name prefix and the `:user_id`
            // segment. Resetting it here (which this did, by analogy
            // with the INGESTER's reset — a different question, about
            // inferring a controller for a bare verb) cost campfire 35
            // of its 78 route helpers: `account_logo` came out `logo` at
            // `/logo`, `user_avatar` came out `avatar` at `/avatar`, and
            // every page linking to one of them 404'd.
            //
            // Note the third line: a scope PATH prefixes outside the
            // parent nesting, which `prefix_path(ns_path, nested_path)`
            // below already gets right.
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
            for (k, v) in defaults {
                let name = k.as_str().to_string();
                child.param_defaults.retain(|(n, _)| n != &name);
                child.param_defaults.push((name, v.clone()));
            }
            for entry in entries {
                collect_flat_routes(entry, out, &child);
            }
        }
    }
}

/// The scope defaults that apply to THIS route's params. A default for
/// a segment the route does not carry is irrelevant here (Rails would
/// put it in the query string; the helper signature is unaffected).
fn defaults_for(ctx: &Ctx, params: &[String]) -> Vec<(String, String)> {
    ctx.param_defaults
        .iter()
        .filter(|(n, _)| params.iter().any(|p| p == n))
        .cloned()
        .collect()
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
    parents: &[Nesting],
    rscope: ResourceScope,
) -> (String, Vec<String>) {
    let Some((innermost, outer)) = parents.split_last() else {
        return (path.to_string(), vec![]);
    };
    // Every level ABOVE the innermost contributes its segment and, if it
    // has one, its id — identically for all three scopes. Only the
    // innermost differs, which is what the match below is about.
    let mut prefix = String::new();
    let mut params: Vec<String> = Vec::new();
    for frame in outer {
        prefix.push('/');
        prefix.push_str(&frame.plural);
        if frame.has_id {
            prefix.push_str(&format!("/:{}_id", frame.singular));
            params.push(format!("{}_id", frame.singular));
        }
    }
    let (parent, parent_plural) = (innermost.singular.as_str(), innermost.plural.as_str());
    let prefix = &prefix;
    // Rails joins route segments with `/` unconditionally. A bare-verb
    // shortcut arrives here already slash-prefixed (`/reply`), but an
    // explicit path string is verbatim source — campfire writes
    // `get "@:message_id"` inside `resources :rooms`, which Rails serves
    // at `/rooms/:room_id/@:message_id`. Concatenating raw glued it onto
    // the id (`/rooms/:room_id@:message_id`) and the route never matched.
    let owned_path;
    let path: &str = if path.starts_with('/') {
        path
    } else {
        owned_path = format!("/{path}");
        &owned_path
    };
    match rscope {
        // `member do get "reply" end` → `/comments/:id/reply` (`:id`, the
        // record's own key — what a controller's `find` reads as
        // `params[:id]`). An already-structured path inside `member`
        // (`get "/comments/:id" => …`, a leading-slash absolute route) is
        // used verbatim, matching Rails' escape from the nesting.
        ResourceScope::Member => {
            if is_bare_child_segment(path) {
                params.push("id".to_string());
                (format!("{prefix}/{parent_plural}/:id{path}"), params)
            } else {
                (path.to_string(), vec![])
            }
        }
        // `collection do get "search" end` → `/photos/search` (no id).
        ResourceScope::Collection => {
            if is_bare_child_segment(path) {
                (format!("{prefix}/{parent_plural}{path}"), params)
            } else {
                (path.to_string(), vec![])
            }
        }
        // Bare verb declared directly in the block, or a nested resource's
        // own actions: Rails nests under the parent's `/:<singular>_id`
        // — unless the parent is SINGULAR, which has no id to nest under.
        ResourceScope::Nested => {
            let mut full = format!("{prefix}/{parent_plural}");
            if innermost.has_id {
                full.push_str(&format!("/:{parent}_id"));
                params.push(format!("{parent}_id"));
            }
            full.push_str(path);
            (full, params)
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
    parent_prefix: &str,
    ns_prefix: &str,
) -> String {
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

    /// A plural-resource nesting frame, the common test shape.
    fn plural(singular: &str, plural: &str) -> Vec<Nesting> {
        vec![Nesting {
            singular: singular.to_string(),
            plural: plural.to_string(),
            has_id: true,
        }]
    }

    #[test]
    fn member_route_nests_under_id() {
        // `member do get "reply" end` in `resources :comments` — the
        // record's own key, so `find_comment` reads `params[:id]`.
        let (path, params) =
            nest_path("/reply", &plural("comment", "comments"), ResourceScope::Member);
        assert_eq!(path, "/comments/:id/reply");
        assert_eq!(params, vec!["id".to_string()]);
    }

    #[test]
    fn member_route_absolute_path_used_verbatim() {
        // `get "/comments/:id" => …` inside a member block escapes nesting.
        let (path, params) = nest_path(
            "/comments/:id",
            &plural("comment", "comments"),
            ResourceScope::Member,
        );
        assert_eq!(path, "/comments/:id");
        assert!(params.is_empty());
    }

    #[test]
    fn collection_route_has_no_id_segment() {
        let (path, params) =
            nest_path("/search", &plural("photo", "photos"), ResourceScope::Collection);
        assert_eq!(path, "/photos/search");
        assert!(params.is_empty());
    }

    #[test]
    fn bare_verb_in_resources_keeps_parent_id() {
        // `post "upvote"` directly in `resources :stories` → `:story_id`.
        let (path, params) =
            nest_path("/upvote", &plural("story", "stories"), ResourceScope::Nested);
        assert_eq!(path, "/stories/:story_id/upvote");
        assert_eq!(params, vec!["story_id".to_string()]);
    }

    #[test]
    fn top_level_route_is_unnested() {
        let (path, params) = nest_path("/login", &[], ResourceScope::Nested);
        assert_eq!(path, "/login");
        assert!(params.is_empty());
    }

    #[test]
    fn constraint_regex_survives_flatten_for_converter() {
        // Two routes share `GET /t/:tag`, distinguished ONLY by a
        // `constraints:` regexp (Lobsters' single- vs multi-tag, #67).
        // The regex must reach FlatRoute.constraints so the roda
        // converter can disambiguate; dropped, the two collapse to
        // duplicate branches with the second unreachable.
        let src = br#"
Rails.application.routes.draw do
  get "/t/:tag" => "home#single_tag", :constraints => { tag: /[^,.\/]+/ }
  get "/t/:tag" => "home#multi_tag"
end
"#;
        let table =
            crate::ingest::ingest_routes(src, "config/routes.rb").expect("routes ingest");
        let mut app = App::default();
        app.routes = table;
        let routes = flatten_routes(&app);

        let single =
            routes.iter().find(|r| r.action.as_str() == "single_tag").expect("single_tag");
        let multi =
            routes.iter().find(|r| r.action.as_str() == "multi_tag").expect("multi_tag");
        assert_eq!(single.path, "/t/:tag");
        assert_eq!(multi.path, "/t/:tag");
        assert_eq!(
            single.constraints,
            vec![("tag".to_string(), "[^,.\\/]+".to_string())],
            "constrained route carries the raw regex source (escapes preserved)"
        );
        assert!(multi.constraints.is_empty(), "unconstrained fallback carries no constraint");
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
