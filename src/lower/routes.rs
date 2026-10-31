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
    /// The path as written, before its optional groups were expanded
    /// (`/top(/:length(/page/:page))` for each of its three variants).
    /// Rails leaves a written route's own parts out of a generated
    /// query, whichever variant it builds.
    pub source_path: String,
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
    /// Whether this route's helper namespace belongs to the current app.
    /// Isolated engine routes share dispatch but expose helpers only on a
    /// mount proxy, so their routes must not seed the host helper registry.
    pub helpers_enabled: bool,
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
    /// `constraints` entries the router CAN enforce, each compiled by
    /// `segment_pattern::compile` into its portable encoded form — a
    /// sequence of literal/char-class items with no backtracking
    /// needed, checkable with plain char comparisons on every target
    /// (`seg_patterns` rides `Route.new`'s `seg_constraints` the way
    /// `int_params` rides its own slot). A route carrying a
    /// `constraints` entry that DIDN'T compile is marked `refused` and
    /// left out of every dispatch table, rather than serve a requirement
    /// it can't actually check (the original bug was exactly that: a
    /// route that claims a restriction but doesn't enforce it).
    pub seg_patterns: Vec<(String, String)>,
    /// `Some((param, regex))` when a requirement on this route is outside
    /// the segment-pattern subset. The route stays out of every dispatch
    /// table (`flatten_routes`) but its URL helper is still generated
    /// (`flatten_routes_for_helpers`): building a link needs no matcher,
    /// and a view that only links to the route must not lose the helper.
    pub refused: Option<(String, String)>,
}

/// Compile every non-digit-class requirement on one concrete route
/// into the router's segment-pattern form, or refuse the WHOLE route
/// (returning the first requirement that failed, with no patterns) when
/// any one of them needs more than the supported subset
/// (`segment_pattern`'s module docs name it).
/// Serving the route with only SOME of its requirements enforced
/// would just reproduce the original bug under a different regex
/// shape, so nothing partial is ever emitted. Ingest reports each
/// refused route as a located `unsupported` (`refused_route_diagnostics`).
fn compile_seg_patterns(
    other_constraints: &[(String, String)],
    path: &str,
) -> (Vec<(String, String)>, Option<(String, String)>) {
    let mut out = Vec::with_capacity(other_constraints.len());
    let mut suffixes: Vec<String> = Vec::new();
    for (name, rx) in other_constraints {
        if let Some(span) = crate::lower::segment_pattern::dot_span(rx) {
            let segments: Vec<&str> = path.split('/').collect();
            let at = segments.iter().position(|s| s.strip_prefix(':') == Some(name.as_str()));
            let last = at.is_some_and(|i| i + 1 == segments.len());
            let spans = segments.iter().filter(|s| s.starts_with('*')).count()
                + out.iter().filter(|(k, _): &&(String, String)| k.starts_with('*')).count();
            // The router spans one run of whole segments: a `.+` param inside a segment (`/@:name`), or beside another span, has no split it can find.
            if at.is_none() || spans > 0 || (!last && !span.slash_follows) {
                return (Vec::new(), Some((name.clone(), rx.clone())));
            }
            out.push((crate::lower::segment_pattern::span_key(name), String::new()));
            if last {
                if span.exts.is_empty() {
                    out.push(("format_suffix".to_string(), crate::lower::segment_pattern::NEVER.to_string()));
                } else {
                    suffixes.extend(span.exts);
                }
            }
            continue;
        }
        match crate::lower::segment_pattern::compile(rx) {
            Some(pattern) => out.push((name.clone(), pattern)),
            None => match crate::lower::segment_pattern::split_format_lookahead(rx)
                .and_then(|(body, exts)| Some((crate::lower::segment_pattern::compile(&body)?, exts)))
            {
                Some((pattern, exts)) => {
                    out.push((name.clone(), pattern));
                    // Only a param the path ends in: one followed by `/` is stopped there by the lookahead, and the format is another segment's.
                    if last_segment_param(path) == Some(name.as_str()) {
                        suffixes.extend(exts);
                    }
                }
                None => return (Vec::new(), Some((name.clone(), rx.clone()))),
            },
        }
    }
    if !suffixes.is_empty() {
        match crate::lower::segment_pattern::compile(&suffixes.join("|")) {
            Some(pattern) => out.push(("format_suffix".to_string(), pattern)),
            None => return (Vec::new(), Some(("format_suffix".to_string(), suffixes.join("|")))),
        }
    }
    (out, None)
}

/// The param a route's path ends in (`/domains/:id` → `id`).
fn last_segment_param(path: &str) -> Option<&str> {
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    last.strip_prefix(':')
}

/// Add a route's `(.:format)` requirement (`dialect::FORMAT_REQUIREMENT`)
/// to its compiled segment patterns, under `format`: the router checks a
/// request's extension against it, and a request with no extension is
/// not held to it. A requirement outside the subset refuses the route,
/// as any other does; a route already refused stays so.
fn with_format_requirement(
    constraints: &indexmap::IndexMap<Symbol, String>,
    (mut seg_patterns, refused): (Vec<(String, String)>, Option<(String, String)>),
) -> (Vec<(String, String)>, Option<(String, String)>) {
    let Some(src) = constraints.get(&Symbol::from(crate::dialect::FORMAT_REQUIREMENT)) else {
        return (seg_patterns, refused);
    };
    if refused.is_some() {
        return (seg_patterns, refused);
    }
    let pattern = if src == crate::dialect::FORMAT_NONE {
        Some(crate::lower::segment_pattern::NEVER.to_string())
    } else {
        crate::lower::segment_pattern::compile(src)
    };
    match pattern {
        Some(p) => {
            seg_patterns.push(("format".to_string(), p));
            (seg_patterns, None)
        }
        None => (Vec::new(), Some(("format".to_string(), src.clone()))),
    }
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
/// Per generated route helper, which of its positional segments are
/// ID-SHAPED — `id` or `<x>_id`, the same rule `routes_to_library`'s
/// `param_ty` uses to decide a helper parameter is an Integer.
///
/// `rewrite_route_helpers` projects a record argument to `.id`
/// shape-directed: an Ivar reaching a `<x>_path` call becomes
/// `@x.id`. That is right for `article_path(@article)` and WRONG for
/// campfire's `join_url(@join_code)`, where `@join_code` holds a
/// string column and the helper's only segment is `:join_code` — the
/// emit called `.id` on a String and every test on that page died.
///
/// The route's own segment is the signal that works. A type stamp is
/// not: the ivar is assigned in `setup` and read in the test method,
/// and nothing types an ivar across that boundary (measured — a
/// `is_scalar_ty` veto over `arg.ty` never fired).
///
/// Keyed by the `_path` spelling, which is what `rewrite_route_helpers`
/// dispatches on after folding `_url` onto its twin. Unnamed routes
/// generate no helper and are skipped. When two routes share an
/// `as_name` (a resources index and create), the first wins — they
/// describe the same path template.
pub fn helper_id_segments(app: &App) -> crate::det::HashMap<String, Vec<bool>> {
    let mut out: crate::det::HashMap<String, Vec<bool>> = crate::det::HashMap::default();
    for r in flatten_routes_for_helpers(app) {
        if !r.named || r.as_name.is_empty() {
            continue;
        }
        let shape: Vec<bool> = r
            .path_params
            .iter()
            .map(|p| p == "id" || p.ends_with("_id"))
            .collect();
        out.entry(format!("{}_path", r.as_name)).or_insert(shape);
    }
    out
}

/// The app's `direct` helpers, as the `RouteHelpers` methods a call names
/// (`fresh_account_logo_path`, `fresh_account_logo_url`). Their bodies
/// are the blocks the app wrote, so a caller's arguments reach them as
/// written.
pub fn direct_helper_names(app: &App) -> crate::det::HashSet<String> {
    app.routes
        .direct_helpers
        .iter()
        .flat_map(|h| [format!("{}_path", h.name.as_str()), format!("{}_url", h.name.as_str())])
        .collect()
}

/// The routes the router dispatches: every flattened route except those
/// refused for a requirement it cannot enforce.
pub fn flatten_routes(app: &App) -> Vec<FlatRoute> {
    let mut out = flatten_route_entries(&app.routes.entries);
    out.retain(|r| r.refused.is_none());
    out
}

/// Every flattened route, refused ones included — for URL-helper
/// generation, which builds a path and never matches one.
pub fn flatten_routes_for_helpers(app: &App) -> Vec<FlatRoute> {
    flatten_route_entries(&app.routes.entries)
}

pub(crate) fn flatten_route_entries(entries: &[RouteSpec]) -> Vec<FlatRoute> {
    let mut out = Vec::new();
    let ctx = Ctx { helpers_enabled: true, ..Ctx::default() };
    for entry in entries {
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
    /// Whether generated route helpers belong to the current table. Isolated
    /// engine helpers live on an engine-specific proxy that is not modeled.
    helpers_enabled: bool,
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
    /// Whether any `resources`/`resource` block encloses this entry —
    /// still true under a `nested do` scope, which folds `parents` into
    /// the path and name prefixes and clears them. Without an `as:`,
    /// Rails names no route there whose action is not a plain word;
    /// outside every resource such a route takes the scope's own prefix.
    in_resource: bool,
}

/// One enclosing resource.
#[derive(Clone, Debug, PartialEq)]
struct Nesting {
    /// Helper-name segment (`user`, `account`; `person` under `as:
    /// :people`); accumulates into the prefix in declaration order and
    /// names a child's `:<singular>_id`.
    singular: String,
    /// Plural helper-name segment: the `as:` name when given, else the
    /// name as written (`users`, `account`).
    plural: String,
    /// Path segment: the name as written, or the resource's `path:`
    /// (`resources :users, path: "people"` nests at `/people/:user_id`).
    segment: String,
    /// Whether this level contributes a `/:<singular>_id` segment.
    ///
    /// FALSE for a singular `resource :account`: Rails routes its
    /// children at `/account/…` with no `:account_id`, because there is
    /// only ever one. Getting this wrong put a phantom id in the middle
    /// of every path under a singular parent (`/account/:account_id/logo`).
    has_id: bool,
    /// The parent's `param:` name (`id` by default). A child nests under
    /// `/:<singular>_<param>` — `resources :tasks, param: :task_id`
    /// gives its children `:task_task_id`, per Rails.
    param: String,
}

impl Ctx {
    /// `scope defaults: { format: :json }` — the response format every
    /// route under the scope answers in unless the path names one.
    /// Rails seeds `params[:format]` from it, so `respond_to` takes the
    /// json branch on a request that sent no Accept header; here it
    /// rides the same `req_format` slot an explicit `format:` on the
    /// route does. campfire's bot API is one such scope, and its
    /// inherited `update` answered the html branch (a redirect) to a
    /// bot's PATCH.
    fn default_format(&self) -> Option<Symbol> {
        self.param_defaults
            .iter()
            .rev()
            .find(|(n, _)| n == "format")
            .map(|(_, v)| Symbol::from(v.as_str()))
    }

    /// `account_bot_` — every enclosing level's singular, in order.
    fn parent_name_prefix(&self) -> String {
        self.parents
            .iter()
            .map(|n| format!("{}_", n.singular))
            .collect()
    }

    /// Where a custom route sits relative to the innermost enclosing
    /// resource, decided once for both its path and its name the way
    /// Rails' `decomposed_match` does: `on:` or a `member`/`collection`
    /// block says so outright, and a bare verb is NESTED in a plural
    /// resource but a MEMBER route of a singular one, which has no id to
    /// nest under. None outside any resource.
    ///
    /// Taking a singular parent's bare verb as nested named it
    /// parent-first (`profile_preview`) where Rails 8.1.4 has
    /// `preview_profile`.
    fn route_scope(&self, written: ResourceScope) -> Option<ResourceScope> {
        let innermost = self.parents.last()?;
        Some(match written {
            ResourceScope::Nested if !innermost.has_id => ResourceScope::Member,
            scope => scope,
        })
    }

    /// Helper name of a custom route whose own part is `part` (from
    /// `route_name_part`), assembled in Rails' `Scope#action_name` order
    /// for its scope (checked with `bin/rails routes`, Rails 8.1.4):
    ///
    ///   member      part, scope prefix, outer singulars, singular   pin_admin_user_note
    ///   collection  part, scope prefix, outer singulars, plural     search_admin_widgets
    ///   nested      scope prefix, every singular, part              admin_author_book_cover
    ///   no resource scope prefix, part                              admin_status
    ///
    /// The scope prefix is the namespace and `scope as:` names only — a
    /// `scope path:` or `module:` is no part of a name.
    fn route_name(&self, part: Option<&str>, scope: Option<ResourceScope>) -> String {
        let parts = match (scope, self.parents.split_last()) {
            (Some(ResourceScope::Member | ResourceScope::Collection), Some((innermost, outer))) => {
                let resource = if scope == Some(ResourceScope::Member) {
                    &innermost.singular
                } else {
                    &innermost.plural
                };
                let outer: String = outer.iter().map(|n| format!("{}_", n.singular)).collect();
                [part.map(str::to_string), Some(format!("{}{outer}{resource}", self.name_prefix))]
            }
            _ => {
                let prefix = format!("{}{}", self.name_prefix, self.parent_name_prefix());
                let prefix = prefix.strip_suffix('_').unwrap_or(&prefix).to_string();
                [Some(prefix), part.map(str::to_string)]
            }
        };
        parts.into_iter().flatten().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_")
    }
}

/// The action of a custom route as Rails reads it for naming: the path
/// as written, when it is a plain word path (`preview`, `contact-info`,
/// `terms/summary`). None for anything carrying a segment, an optional
/// group or an extension (`photo/:size`, `/feed(.:format)`,
/// `robots.txt`) — Rails names such a route only from an explicit `as:`.
fn written_action(path: &str) -> Option<&str> {
    let plain = !path.is_empty()
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/'));
    plain.then_some(path)
}

/// Rails' `canonical_action?`: one of the actions a resource defines
/// for itself, declared on its member or collection — where it is the
/// resource's own action, not a new one beside it.
fn canonical_action(action: &str, scope: Option<ResourceScope>) -> bool {
    matches!(scope, Some(ResourceScope::Member | ResourceScope::Collection))
        && ["index", "create", "new", "show", "update", "destroy"].contains(&action)
}

/// The part a custom route contributes to its helper name — Rails'
/// `prefix_name_for_action`: the `as:` name, else the action, with
/// hyphens and slashes made underscores (`contact-info` → `contact_info`).
/// A canonical action adds nothing (`collection { get :index }` is
/// named like the resource's own `index`), whichever way it is written.
fn route_name_part(
    as_name: Option<&str>,
    action: Option<&str>,
    scope: Option<ResourceScope>,
) -> Option<String> {
    let part = match (as_name, action) {
        (Some(name), _) => name,
        (None, Some(action)) if !canonical_action(action, scope) => action,
        _ => return None,
    };
    let part = part
        .replace('-', "_")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    (!part.is_empty()).then_some(part)
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
        RouteSpec::Explicit {
            method,
            path,
            controller,
            action,
            as_name,
            scope,
            constraints,
            action_symbol,
        } => {
            let constraints_map = constraints;
            // `:format => "rss"` rides the constraints map at ingest
            // (it shapes the request, not the routing triple); surface
            // it as the route's forced response format.
            let forced_format = constraints
                .get(&Symbol::from("format"))
                .map(|f| Symbol::from(f.as_str()));
            let route_scope = ctx.route_scope(*scope);
            // A canonical action written as a symbol IS the resource's own
            // route, at its own path: Rails 8.1.4 serves
            // `collection { get :index }` at `/widgets` and a singular
            // resource's `member { post :update }` at `/account`. Only the
            // string spelling (`get "index"`) is taken as a path, and
            // keeps its `/index`. Appending the action put the symbol form
            // at `/widgets/index` — where `/widgets/:id` answers it as a
            // show — under the name of the resource's own helper.
            let own_path = *action_symbol && canonical_action(path, route_scope);
            let (nested, base_params) = nest_path(
                if own_path { "" } else { path },
                &ctx.parents,
                route_scope.unwrap_or(*scope),
            );
            let full_path = prefix_path(&ctx.ns_path, &nested);
            // Empty only for a resource's own path at the very top (a
            // canonical action on `resources :x, path: "/"`), which is `/`.
            let full_path = if full_path.is_empty() { "/".to_string() } else { full_path };
            // Rails optional `(…)` segments (`get "/s/:id/(:title)"`) match
            // whether or not the segment is present; expand them into
            // concrete routes (`/s/:id/:title` and `/s/:id`) so the
            // segment-count router matches both. Paths with no optional
            // group yield a single unchanged entry.
            let variants = expand_optional_paths(&full_path);
            // The helper name comes from the route's SCOPE and its own
            // part (`as:`, else the action as written), never from the
            // full path: a `scope path:` or a resource's `path:` moves
            // the URL only, and which way the parts read depends on the
            // scope. Measured against Rails 8.1 inside `resources :rooms`:
            //
            //   get "@:id", as: :at_message        → room_at_message
            //   get :preview, on: :member, as: :peek → peek_room
            //   get :recent, on: :collection, as: :fresh → fresh_rooms
            //
            // A nested route reads parent-first (it is a thing
            // BELONGING to the room); a member/collection route reads
            // verb-first (it is an ACTION on the room). Prefixing
            // unconditionally cost campfire `room_at_message` and
            // `room_bot_messages`; naming from the path cost a bare verb
            // every resource above its innermost one (`book_cover` for
            // Rails' `author_book_cover`).
            //
            // Without an `as:`, Rails names nothing for a route inside a
            // resource whose action is not a plain word path, and drops
            // a name no target can declare (`/404`) or one an earlier
            // route already took — so a repeated verb (`get :preview`, then
            // `post :preview`) shares the first route's helper. An unnamed
            // route keeps the legacy action-name fallback in `as_name`
            // for consumers that key on it, but the helper generator
            // skips it.
            let action_part = written_action(path);
            let derived = match as_name.as_ref() {
                Some(s) => Some(ctx.route_name(
                    route_name_part(Some(s.as_str()), None, route_scope).as_deref(),
                    route_scope,
                )),
                None if action_part.is_none() && ctx.in_resource => None,
                None => {
                    let n = ctx.route_name(
                        route_name_part(None, action_part, route_scope).as_deref(),
                        route_scope,
                    );
                    let declarable =
                        n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
                    (declarable && !out.iter().any(|r| r.named && r.as_name == n)).then_some(n)
                }
            };
            let (derived_name, named) = match derived {
                Some(n) if !n.is_empty() => (n, true),
                _ => (action.as_str().to_string(), false),
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
                // Any non-digit requirement outside the router's
                // supported segment-pattern subset refuses the WHOLE
                // route rather than serve it unenforced — a route
                // that advertises `[^@/.]+` but can't actually check
                // it is exactly the original bug.
                let (seg_patterns, refused) =
                    with_format_requirement(constraints_map, compile_seg_patterns(&constraints, &vpath));
                out.push(FlatRoute {
                    method: method.clone(),
                    source_path: full_path.clone(),
                    path: vpath,
                    controller: qualify_controller(&ctx.module_prefix, controller),
                    action: action.clone(),
                    as_name: if ctx.helpers_enabled { derived_name.clone() } else { String::new() },
                    named: named && i == 0 && ctx.helpers_enabled,
                    helpers_enabled: ctx.helpers_enabled,
                    format: forced_format.clone().or_else(|| ctx.default_format()),
                    required_params,
                    param_defaults: defaults_for(ctx, &params),
                    path_params: params,
                    int_params,
                    constraints,
                    seg_patterns,
                    refused,
                });
            }
        }
        // A Root with no target is not a route (the ingester now
        // refuses to build one, #82); skipping it here keeps a
        // hand-built or deserialized table from emitting `:` as a
        // controller symbol.
        RouteSpec::Root { target, .. } if target.is_empty() => {}
        RouteSpec::Root { target, as_name } => {
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
            // `camelize_path`: a namespaced target (`root to:
            // "rails/health#show"`, the Rails 7.1+ health check) names
            // `Rails::HealthController`; `camelize` kept the slash and
            // the dispatch read `Rails/healthController.new`.
            let controller_class = format!(
                "{}{}Controller",
                ctx.module_prefix,
                naming::camelize_path(&controller_name)
            );
            let path =
                if ctx.ns_path.is_empty() { "/".to_string() } else { ctx.ns_path.clone() };
            // Rails applies the scope `as` prefix to an explicit root
            // `as:` (`namespace :admin do root … as: :home end` →
            // `admin_home`), matching Explicit's name_prefix handling.
            let helper = as_name
                .as_ref()
                .map(|s| format!("{}{}", ctx.name_prefix, s.as_str()))
                .unwrap_or_else(|| format!("{}root", ctx.name_prefix));
            out.push(FlatRoute {
                method: HttpMethod::Get,
                source_path: path.clone(),
                path,
                controller: ClassId(Symbol::from(controller_class)),
                action: Symbol::from(action_name),
                as_name: if ctx.helpers_enabled { helper } else { String::new() },
                path_params: vec![],
                param_defaults: vec![],
                named: ctx.helpers_enabled,
                helpers_enabled: ctx.helpers_enabled,
                format: None,
                required_params: 0,
                int_params: vec![],
                constraints: vec![],
                seg_patterns: vec![],
                refused: None,
            });
        }
        RouteSpec::Resources {
            name,
            only,
            except,
            nested,
            singular,
            as_name,
            controller,
            param,
            path: path_segment,
            constraints: resource_constraints,
        } => {
            // `path:` moves the URL segment only; the helpers and the
            // controller below still come from `name`.
            let segment = path_segment.as_deref().unwrap_or(name.as_str());
            // `path: "/"` (an empty segment) mounts the resource at its
            // scope's root: `/ping`, not `//ping`.
            let resource_path = slash_segment(segment);
            // `param: :task_id` renames the member segment (#84). The
            // action table is written with `:id`; substitute here so
            // the path, the helper's param list and the controller's
            // `params[:task_id]` all agree.
            let id_param = param.as_ref().map(|p| p.as_str()).unwrap_or("id");
            // `resource :profile` still routes to the *plural*
            // controller (`ProfilesController`), per Rails.
            let controller_stem = if *singular {
                naming::camelize(&naming::pluralize_snake(name.as_str()))
            } else {
                naming::camelize(name.as_str())
            };
            // `controller: "messages/by_bots"` names the class outright
            // — `Messages::ByBotsController`, still under any enclosing
            // `scope module:`. Only the class moves; `controller_stem`
            // above stays the source of the path and the helper names.
            let controller_class = match controller {
                Some(c) => ClassId(Symbol::from(format!(
                    "{}{}",
                    ctx.module_prefix,
                    c.split('/')
                        .map(naming::camelize)
                        .collect::<Vec<_>>()
                        .join("::")
                        + "Controller"
                ))),
                None => ClassId(Symbol::from(format!(
                    "{}{}Controller",
                    ctx.module_prefix, controller_stem
                ))),
            };
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

            // Rails evaluates the resource block before adding its defaults.
            // Register children in source order first: otherwise `/:id` swallows
            // a collection action such as `/transactions/bulk_update`, and a
            // default member route wins over an explicit override of that path.
            let child_ctx = Ctx {
                parents: {
                    let mut p = ctx.parents.clone();
                    // A child reads the parent's HELPER names, so an
                    // `as:` alias reaches it too: Rails 8.1.4 nests
                    // `resources :users, as: :people { resources :notes }`
                    // as `person_notes` at `/users/:person_id/notes`, and
                    // names a member route `pin_person`. Only the path
                    // segment stays `users`.
                    p.push(Nesting {
                        singular: helper_singular.clone(),
                        plural: helper_plural.clone(),
                        segment: segment.to_string(),
                        has_id: !*singular,
                        param: id_param.to_string(),
                    });
                    p
                },
                in_resource: true,
                ..ctx.clone()
            };
            for child in nested {
                collect_flat_routes(child, out, &child_ctx);
            }
            // Every route registered before the defaults — the block's
            // own among them. A default action whose name one of those
            // already took is left unnamed, as Rails leaves it: the
            // block's `collection { get :index }` keeps `widgets_path`.
            let earlier = out.len();

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
                let member = suffix.replace(":id", &format!(":{id_param}"));
                // Empty for an index or show mounted with `path: "/"`; it
                // stays empty through the nesting (`/account`, not
                // `/account/`) and is the root `/` only at the very top.
                let path = format!("{resource_path}{member}");
                let (nested_path, nested_params) =
                    nest_path(&path, &ctx.parents, ResourceScope::Nested);
                let full_path = prefix_path(&ctx.ns_path, &nested_path);
                let full_path = if full_path.is_empty() { "/".to_string() } else { full_path };
                // A scope PATH can carry a dynamic segment of its own
                // (`scope path: ":bot_key"`), and it is a helper
                // argument like any other. `nest_path` only knows the
                // nesting's params, so read the scope's off `ns_path` —
                // they sit in FRONT, which is where `prefix_path` just
                // put them.
                let mut params: Vec<String> = Vec::new();
                extract_path_params(&ctx.ns_path, &mut params);
                params.retain(|p| !nested_params.iter().any(|q| q == p));
                params.extend(nested_params);
                if suffix.contains(":id") && !params.iter().any(|p| p == id_param) {
                    params.push(id_param.to_string());
                }
                // Inherited block-level `constraints(id: /.../) do …
                // end` (propagated onto this `Resources` node at
                // ingest) applies only to the actions whose path
                // actually carries the constrained param — `show`/
                // `edit`/`update`/`destroy` for the common `id` case,
                // never `index`/`new`/`create`. Same digit-class split
                // as the `Explicit` arm above.
                let int_params: Vec<String> = resource_constraints
                    .iter()
                    .filter(|(name, rx)| {
                        params.iter().any(|p| p == name.as_str()) && digit_class_regex(rx)
                    })
                    .map(|(name, _)| name.as_str().to_string())
                    .collect();
                let other_constraints: Vec<(String, String)> = resource_constraints
                    .iter()
                    .filter(|(name, rx)| {
                        params.iter().any(|p| p == name.as_str()) && !digit_class_regex(rx)
                    })
                    .map(|(name, rx)| (name.as_str().to_string(), rx.clone()))
                    .collect();
                let as_name = resource_as_name(
                    action_name,
                    &helper_singular,
                    &helper_plural,
                    &ctx.parent_name_prefix(),
                    &ctx.name_prefix,
                );
                // Same fail-closed rule as the `Explicit` arm: a
                // requirement outside the supported subset refuses
                // this action's route rather than serve it unenforced.
                let (seg_patterns, refused) =
                    with_format_requirement(resource_constraints, compile_seg_patterns(&other_constraints, &full_path));
                out.push(FlatRoute {
                    method: method.clone(),
                    source_path: full_path.clone(),
                    path: full_path.clone(),
                    controller: controller_class.clone(),
                    action: Symbol::from(action_name),
                    as_name: if ctx.helpers_enabled { as_name.clone() } else { String::new() },
                    required_params: params.len(),
                    param_defaults: defaults_for(ctx, &params),
                    path_params: params.clone(),
                    named: ctx.helpers_enabled
                        && !out[..earlier].iter().any(|r| r.named && r.as_name == as_name),
                    helpers_enabled: ctx.helpers_enabled,
                    format: ctx.default_format(),
                    int_params: int_params.clone(),
                    constraints: other_constraints.clone(),
                    seg_patterns: seg_patterns.clone(),
                    refused: refused.clone(),
                });
                // Rails routes `update` on BOTH `PATCH` and `PUT` — the
                // verb changed in Rails 4 and the older one was kept, so
                // both reach the same action. campfire's controller
                // tests write `put account_user_url(...)`, and a table
                // carrying only the PATCH row answered "no route
                // matches" for a request Rails routes fine.
                //
                // `named: false`: the pair is ONE route as far as
                // helpers go, and a second named row would either
                // duplicate `<x>_path` or lose the dedupe race with it.
                // Only the dispatch table needs both.
                if action_name == "update" {
                    out.push(FlatRoute {
                        method: HttpMethod::Put,
                        source_path: full_path.clone(),
                        path: full_path,
                        controller: controller_class.clone(),
                        action: Symbol::from(action_name),
                        as_name: if ctx.helpers_enabled { as_name } else { String::new() },
                        required_params: params.len(),
                        param_defaults: defaults_for(ctx, &params),
                        path_params: params,
                        named: false,
                        helpers_enabled: ctx.helpers_enabled,
                        format: ctx.default_format(),
                        int_params,
                        constraints: other_constraints,
                        seg_patterns,
                        refused,
                    });
                }
            }
        }
        RouteSpec::Scope { path, module, as_prefix, defaults, nest, suppress_helpers, entries } => {
            let mut child = ctx.clone();
            if *suppress_helpers {
                child.helpers_enabled = false;
            }
            // `nested do … end`: MATERIALIZE the pending parent nesting
            // now, so this scope's own `path`/`as` land INSIDE it. The
            // ordinary path below prefixes `ns_path` in FRONT of the
            // nesting (`scope :extra` in the comment below is
            // `/extra/users/:user_id/badge`), which is right for a
            // scope and wrong for a `nested` one — campfire's bot API
            // is `/rooms/:room_id/:bot_key/messages`, not
            // `/:bot_key/rooms/:room_id/messages`.
            if *nest && !child.parents.is_empty() {
                let (nested_prefix, _) =
                    nest_path("", &child.parents, ResourceScope::Nested);
                // `nest_path` slash-prefixes a bare path, so the empty
                // one comes back with a trailing `/`.
                let nested_prefix = nested_prefix.trim_end_matches('/');
                child.ns_path = prefix_path(&child.ns_path, nested_prefix);
                child.name_prefix =
                    format!("{}{}", child.name_prefix, child.parent_name_prefix());
                child.parents.clear();
            }
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
    // A root scope (`scope path: "/"`) contributes nothing, and a scope
    // never ends in a slash: Rails normalizes `//new` to `/new`.
    let ns = ns.trim_end_matches('/');
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
    // The synthesized redirect controller is one class for the whole
    // app: a `to: redirect(...)` written inside a `namespace` would
    // otherwise be qualified into a class nobody defines.
    if controller.0.as_str() == crate::ingest::routes::REDIRECT_CONTROLLER {
        return controller.clone();
    }
    // `devise_for`'s default (no `controllers:` override) is a
    // placeholder, not a qualified class: Devise substitutes the
    // enclosing scope's module for its own `"devise"` default
    // (`Devise::Mapping#default_controllers`) rather than nesting under
    // it, which the generic concatenation below would do.
    if let Some(resolved) =
        crate::ingest::devise_routes::resolve_default_controller(module_prefix, controller.0.as_str())
    {
        return ClassId(Symbol::from(resolved));
    }
    // A leading `::` is a target written with a leading slash
    // (`to: "/errors#routing"`): Rails resolves it from the top level,
    // outside the enclosing `namespace`. `devise_for … controllers:`
    // overrides are marked the same way — Devise resets the scope's
    // module before applying them, so no enclosing scope re-qualifies
    // them either.
    if let Some(absolute) = controller.0.as_str().strip_prefix("::") {
        return ClassId(Symbol::from(absolute));
    }
    if module_prefix.is_empty() {
        controller.clone()
    } else {
        ClassId(Symbol::from(format!("{module_prefix}{}", controller.0.as_str())))
    }
}

/// `/<segment>`, or nothing for the empty segment of a resource mounted
/// at its scope's root (`resources :x, path: "/"`).
fn slash_segment(segment: &str) -> String {
    if segment.is_empty() { String::new() } else { format!("/{segment}") }
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
        prefix.push_str(&slash_segment(&frame.segment));
        if frame.has_id {
            prefix.push_str(&format!("/:{}_{}", frame.singular, frame.param));
            params.push(format!("{}_{}", frame.singular, frame.param));
        }
    }
    let (parent, parent_plural) = (innermost.singular.as_str(), slash_segment(&innermost.segment));
    let prefix = &prefix;
    // Rails joins route segments with `/` unconditionally. A bare-verb
    // shortcut arrives here as written (`reply` or `/reply`), and an
    // explicit path string is verbatim source too — campfire writes
    // `get "@:message_id"` inside `resources :rooms`, which Rails serves
    // at `/rooms/:room_id/@:message_id`. Concatenating raw glued it onto
    // the id (`/rooms/:room_id@:message_id`) and the route never matched.
    let owned_path;
    let path: &str = if path.is_empty() || path.starts_with('/') {
        path
    } else {
        owned_path = format!("/{path}");
        &owned_path
    };
    match rscope {
        // `member do get "reply" end` → `/comments/:id/reply` (`:id`, the
        // record's own key — what a controller's `find` reads as
        // `params[:id]`). Rails prepends the member scope to EVERY path
        // written inside the block, a structured one included:
        // `get "pages/:page"` is `/comments/:id/pages/:page` and even a
        // leading-slash `get "/comments/:id" => …` comes out as
        // `/comments/:id/comments/:id` (measured against Rails 8.1).
        // A SINGULAR parent has no id, so neither do its member routes:
        // `resource :profile { member { get :preview } }` is
        // `/profile/preview` (Rails 8.1.4), like its own `show`.
        ResourceScope::Member => {
            let mut full = format!("{prefix}{parent_plural}");
            if innermost.has_id {
                full.push_str(&format!("/:{}", innermost.param));
                params.push(innermost.param.clone());
            }
            full.push_str(path);
            (full, params)
        }
        // `collection do get "search" end` → `/photos/search` (no id);
        // `get "/(:name)"` → `/photos(/:name)`, never a bare `/:name`.
        ResourceScope::Collection => (format!("{prefix}{parent_plural}{path}"), params),
        // Bare verb declared directly in the block, or a nested resource's
        // own actions: Rails nests under the parent's `/:<singular>_id`
        // — unless the parent is SINGULAR, which has no id to nest under.
        ResourceScope::Nested => {
            let mut full = format!("{prefix}{parent_plural}");
            if innermost.has_id {
                full.push_str(&format!("/:{parent}_{}", innermost.param));
                params.push(format!("{parent}_{}", innermost.param));
            }
            full.push_str(path);
            (full, params)
        }
    }
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
            segment: plural.to_string(),
            has_id: true,
            param: "id".to_string(),
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
    fn member_route_structured_path_still_nests() {
        // Rails prepends the member scope even to a leading-slash path:
        // `get "/comments/:id" => …` inside `member do` is served at
        // `/comments/:id/comments/:id`.
        let (path, params) = nest_path(
            "/comments/:id",
            &plural("comment", "comments"),
            ResourceScope::Member,
        );
        assert_eq!(path, "/comments/:id/comments/:id");
        assert_eq!(params, vec!["id".to_string()]);
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
