//! `config/routes.rb` — parse the `Rails.application.routes.draw do … end`
//! DSL into a `RouteTable`. Recognizes verb shortcuts (`get`/`post`/…),
//! `root`, `resources`/`resource`, `namespace`/`scope`, `concern`/`concerns`,
//! and the file-inclusion forms — `draw(:name)`, `load(path)`,
//! `instance_eval(File.read(path))` — over split files under
//! `config/routes/`.
//!
//! Recovery discipline: in survey mode an unsupported DSL construct
//! (`use_doorkeeper`, …) records a gap and drops
//! that one entry — the rest of the table still flattens. In strict
//! mode it still fails loud so the fixture that introduces a new form
//! forces a recognizer. Not-modeled ≠ absent: a dropped entry is a
//! ledger line, never a silently empty route table. Engine mounts recover
//! in every mode with a located error carried on the route table; strict
//! emission refuses it unless explicitly overridden.
//!
//! Devise's wrappers (`authenticated` / `unauthenticated` /
//! `devise_scope`) flatten like `constraints` — nested routes are kept,
//! auth is not enforced. `devise_for` expands a static route/helper
//! table (sessions / registrations / passwords / confirmations) from
//! the resource name and optional `controllers:` overrides; it does
//! not claim Warden or Devise controller runtime.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use ruby_prism::Node;

use crate::dialect::{DirectHelper, HttpMethod, ResourceScope, RouteSpec, RouteTable};
use crate::naming::camelize;
use crate::{ClassId, Symbol};
use crate::diagnostic::Diagnostic;
use crate::span::Span;

use super::util::{
    constant_id_str, constant_path_of, find_call_named, flatten_statements, string_value, symbol_list_value,
    symbol_or_string_value, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_routes(source: &[u8], file: &str) -> IngestResult<RouteTable> {
    ingest_routes_with_draws(source, file, &HashMap::new())
}

/// `draws` maps a route-file key to the split file Rails loads into the
/// DSL: key → (source, path). The key is the file's path under
/// `config/routes/` without the extension (`draw(:admin)` →
/// `admin`; `load(root.join("config", "routes", "services",
/// "internal.rb"))` → `services/internal`). The app ingester reads the
/// directory; tests pass maps directly.
pub fn ingest_routes_with_draws(
    source: &[u8],
    file: &str,
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<RouteTable> {
    ingest_routes_with_dsl(source, file, draws, &HashSet::new())
}

/// [`ingest_routes_with_draws`] plus the block-taking methods an app
/// added to the routing DSL: `block_wrappers` names methods of a module
/// the app prepends into `ActionDispatch::Routing::Mapper`
/// (`routing_method :main_pod do … end`) — each is a scope that runs its
/// block in the enclosing mapper without changing any path.
pub fn ingest_routes_with_dsl(
    source: &[u8],
    file: &str,
    draws: &HashMap<String, (Vec<u8>, String)>,
    block_wrappers: &HashSet<String>,
) -> IngestResult<RouteTable> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let cx = Ctx {
        draws,
        wrappers: block_wrappers,
        concerns: RefCell::new(HashMap::new()),
        active: RefCell::new(Vec::new()),
        hoisted: RefCell::new(Vec::new()),
        diagnostics: RefCell::new(Vec::new()),
        mount_scope_depth: Cell::new(0),
    };

    // Every `Rails.application.routes.draw do … end` in the file: Rails
    // appends each to the same route set, and a components-style tree
    // concatenates several. An ENGINE's route set
    // (`RetailComponent::Engine.routes.draw`) is a different set — its
    // helpers live on the engine, not on the app's `url_helpers` — so it
    // is not folded into this table.
    let mut draw_calls = top_level_app_draws(&root);
    if draw_calls.is_empty() {
        // A draw nested inside something else (a conditional, a
        // wrapper) — the first one found, as before.
        if let Some(call) = find_call_named(&root, "draw") {
            draw_calls.push(call);
        }
    }

    let mut entries = Vec::new();
    let mut direct_helpers = Vec::new();
    for draw_call in &draw_calls {
        let Some(block_node) = draw_call.block() else { continue };
        let Some(block) = block_node.as_block_node() else { continue };
        if let Some(body) = block.body() {
            entries.extend(ingest_route_body(body, file, None, &cx)?);
            // Collected by a second walk rather than threaded through the
            // recursive entry ingest: a `direct` is not a RouteSpec (it adds no
            // path), so it has nowhere to ride in the `entries` return, and
            // every recursive arm would otherwise need a mutable accumulator
            // for a construct that appears a handful of times per app.
            if let Some(body) = block.body() {
                direct_helpers.extend(collect_direct_helpers(&body, file)?);
            }
        }
    }
    // A `load`ed file carries its own `Rails.application.routes.draw`,
    // which starts a fresh mapper: its routes sit at the top of the
    // table whatever scope the `load` call was written inside.
    entries.extend(cx.hoisted.take());

    Ok(RouteTable { entries, direct_helpers, redirects: redirect_sink::drain(), diagnostics: cx.diagnostics.take() })
}

/// The `draw do … end` calls that are statements of the program itself
/// and draw the APPLICATION's routes: `Rails.application.routes.draw`
/// (or any `<something>.routes.draw` whose receiver is not an engine).
fn top_level_app_draws<'pr>(root: &Node<'pr>) -> Vec<ruby_prism::CallNode<'pr>> {
    let Some(program) = root.as_program_node() else { return Vec::new() };
    program
        .statements()
        .body()
        .iter()
        .filter_map(|stmt| stmt.as_call_node())
        .filter(|call| is_app_route_draw(call))
        .collect()
}

/// `<recv>.routes.draw do … end` where `<recv>` is not an `…::Engine`.
fn is_app_route_draw(call: &ruby_prism::CallNode<'_>) -> bool {
    if constant_id_str(&call.name()) != "draw" || call.block().is_none() {
        return false;
    }
    let Some(recv) = call.receiver() else { return false };
    let Some(routes) = recv.as_call_node() else { return false };
    if constant_id_str(&routes.name()) != "routes" {
        return false;
    }
    let Some(owner) = routes.receiver() else { return false };
    let is_engine = owner
        .as_constant_path_node()
        .and_then(|p| p.name())
        .map(|n| constant_id_str(&n).ends_with("Engine"))
        .unwrap_or(false)
        || owner
            .as_constant_read_node()
            .map(|c| constant_id_str(&c.name()).ends_with("Engine"))
            .unwrap_or(false);
    !is_engine
}

/// What the DSL walk carries besides the syntax in front of it.
struct Ctx<'a> {
    draws: &'a HashMap<String, (Vec<u8>, String)>,
    /// App-defined block-taking DSL methods (see [`ingest_routes_with_dsl`]).
    wrappers: &'a HashSet<String>,
    /// `concern :name do |options| … end`, by name. A concern is a macro
    /// re-run at each `concerns :name` call site, so its body is kept as
    /// source and re-read there.
    concerns: RefCell<HashMap<String, Concern>>,
    /// Route files currently being included, so a file that loads itself
    /// (directly or through another) ends the walk instead of the stack.
    active: RefCell<Vec<String>>,
    /// Entries of `load`ed files that drew at top level.
    hoisted: RefCell<Vec<RouteSpec>>,
    /// Located errors recovered while retaining sibling routes.
    diagnostics: RefCell<Vec<Diagnostic>>,
    /// Fixed runtime mounts cannot inherit path/constraint DSL wrappers.
    mount_scope_depth: Cell<usize>,
}

impl Ctx<'_> {
    /// Restore the enclosing mount context even when a nested walk fails.
    fn with_mount_scope<T>(&self, depth: usize, walk: impl FnOnce() -> T) -> T {
        let previous = self.mount_scope_depth.replace(depth);
        let result = walk();
        self.mount_scope_depth.set(previous);
        result
    }
}

/// The runtime supplies only the top-level `/cable` endpoint. Accept the
/// generated Rails hashrocket form and its equivalent `at:` spelling.
fn runtime_cable_mount(call: &ruby_prism::CallNode<'_>) -> bool {
    fn cable_server(node: &Node<'_>) -> bool {
        let Some(server) = node.as_call_node() else { return false };
        constant_id_str(&server.name()) == "server"
            && server.arguments().is_none()
            && server.block().is_none()
            && server.receiver().and_then(|r| constant_path_of(&r))
                .is_some_and(|parts| parts == ["ActionCable"])
    }
    let Some(args) = call.arguments() else { return false };
    let mut server = false;
    let mut path = None;
    for arg in args.arguments().iter() {
        if cable_server(&arg) {
            server = true;
            continue;
        }
        let Some(hash) = arg.as_keyword_hash_node() else { return false };
        for item in hash.elements().iter() {
            let Some(assoc) = item.as_assoc_node() else { return false };
            if cable_server(&assoc.key()) {
                server = true;
                path = string_value(&assoc.value());
            } else if symbol_value(&assoc.key()).as_deref() == Some("at") {
                path = string_value(&assoc.value());
            } else {
                return false;
            }
        }
    }
    server && path.as_deref() == Some("/cable") && call.block().is_none()
}

struct Concern {
    /// The block parameter the options hash is bound to (`|options|`).
    param: Option<String>,
    /// The block body, as source; `None` for an empty concern.
    body: Option<Vec<u8>>,
    file: String,
}

/// Every `direct :name do |…| … end` in the draw block, at any nesting
/// depth (Rails accepts one inside a `namespace`/`scope` too).
///
/// The block's parameters become the helper's, and its body is ingested
/// as an ordinary Expr — the whole point is that a `direct` body is
/// arbitrary Ruby, so nothing here tries to interpret it. The
/// `route_for` call it evaluates to is resolved later, at lowering,
/// where the flattened route table is available.
fn collect_direct_helpers(node: &Node<'_>, file: &str) -> IngestResult<Vec<DirectHelper>> {
    let mut out = Vec::new();
    collect_direct_helpers_into(node, file, &mut out)?;
    Ok(out)
}

fn collect_direct_helpers_into(
    node: &Node<'_>,
    file: &str,
    out: &mut Vec<DirectHelper>,
) -> IngestResult<()> {
    if let Some(call) = node.as_call_node() {
        if constant_id_str(&call.name()) == "direct" {
            if let Some(helper) = ingest_direct_helper(&call, file)? {
                out.push(helper);
                // Don't descend into a `direct` body — a `route_for`
                // inside it is the helper's content, not another route.
                return Ok(());
            }
        }
    }
    // Explicit descent, matching how every other walk in this tree
    // recurses (prism's Node exposes no generic child visitor). Inside a
    // routes file a `direct` can only be nested in another block-taking
    // DSL call — `namespace`, `scope`, `resources` — so statements and
    // call blocks are the whole path.
    if let Some(stmts) = node.as_statements_node() {
        for stmt in stmts.body().iter() {
            collect_direct_helpers_into(&stmt, file, out)?;
        }
        return Ok(());
    }
    if let Some(call) = node.as_call_node() {
        if let Some(body) = call
            .block()
            .and_then(|b| b.as_block_node())
            .and_then(|b| b.body())
        {
            collect_direct_helpers_into(&body, file, out)?;
        }
    }
    Ok(())
}

fn ingest_direct_helper(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<DirectHelper>> {
    let Some(name) = call
        .arguments()
        .and_then(|args| args.arguments().iter().next().and_then(|a| symbol_value(&a)))
    else {
        return Ok(None);
    };
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else {
        return Ok(None);
    };
    let params: Vec<Symbol> = block
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|p| p.parameters())
        .map(|pn| {
            pn.requireds()
                .iter()
                .filter_map(|r| {
                    r.as_required_parameter_node()
                        .map(|rp| Symbol::from(constant_id_str(&rp.name())))
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(body) = block.body() else {
        return Ok(None);
    };
    let body = super::expr::ingest_expr(&body, file)?;
    Ok(Some(DirectHelper { name: Symbol::from(name), params, body }))
}

/// Walk the statements inside a `routes.draw do ... end` block (or a
/// nested `resources :x do ... end` block) and collect their `RouteSpec`
/// entries. Recognized forms: verb shortcuts, `root "c#a"`,
/// `resources`/`resource`, `namespace`/`scope`, and `draw(:name)`.
/// `parent` carries the enclosing `resources :<name>` (its plural name)
/// so bare-verb member/nested shortcuts (`get "suggest"` with no `to:`)
/// can infer their controller; `None` at the top level.
fn ingest_route_body(
    body: Node<'_>,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    ingest_route_stmts(flatten_statements(body).into_iter(), file, parent, cx)
}

fn ingest_route_stmts<'pr>(
    stmts: impl Iterator<Item = Node<'pr>>,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    let mut entries = Vec::new();
    for stmt in stmts {
        // A conditional around routes (`if Rails.env.development?`) is
        // decided at boot, not here: both arms are walked, so the
        // helpers of every environment's table exist in the model.
        if let Some(cond) = stmt.as_if_node() {
            let mut arms: Vec<Node<'pr>> = Vec::new();
            if let Some(body) = cond.statements() {
                arms.extend(body.body().iter());
            }
            if let Some(rest) = cond.subsequent() {
                arms.push(rest);
            }
            entries.extend(cx.with_mount_scope(cx.mount_scope_depth.get() + 1, || {
                ingest_route_stmts(arms.into_iter(), file, parent, cx)
            })?);
            continue;
        }
        if let Some(cond) = stmt.as_unless_node() {
            let mut arms: Vec<Node<'pr>> = Vec::new();
            if let Some(body) = cond.statements() {
                arms.extend(body.body().iter());
            }
            if let Some(rest) = cond.else_clause() {
                arms.push(rest.as_node());
            }
            entries.extend(cx.with_mount_scope(cx.mount_scope_depth.get() + 1, || {
                ingest_route_stmts(arms.into_iter(), file, parent, cx)
            })?);
            continue;
        }
        if let Some(rest) = stmt.as_else_node() {
            if let Some(body) = rest.statements() {
                entries.extend(ingest_route_stmts(body.body().iter(), file, parent, cx)?);
            }
            continue;
        }
        // `prefix = options[:prefix] || ""` at the top of a concern.
        if let Some(write) = stmt.as_local_variable_write_node() {
            let value = eval::value(&write.value()).unwrap_or(eval::RVal::Unknown);
            eval::bind(constant_id_str(&write.name()), value);
            continue;
        }
        let Some(call) = stmt.as_call_node() else { continue };
        // `Dir.glob('rest_routes/**/*.rb', base: 'config/routes').each
        // do |r| draw(r.sub(/\.rb$/, '')) end` (and `Dir[...]`) — the
        // idiom a Mastodon-class app uses to mass-`draw` a whole
        // directory instead of one call per file (Procore draws 1,561
        // files this way). Checked BEFORE the receiver-skip below: that
        // skip exists to avoid re-finding the outer `Rails.application
        // .routes.draw` as a nested call, but `Dir.glob(...).each`
        // legitimately has a receiver (`Dir.glob(...)`) and would
        // otherwise fall through it and vanish with no ledger entry.
        if constant_id_str(&call.name()) == "each" {
            if let Some(recv) = call.receiver() {
                if let Some(pattern) = dir_glob_routes_pattern(&recv) {
                    match ingest_glob_draw_each(&call, &pattern, file, cx) {
                        Ok(new_entries) => entries.extend(new_entries),
                        Err(err) if super::survey::is_active() => super::survey::record(&err),
                        Err(err) => return Err(err),
                    }
                    continue;
                }
            }
        }

        if call.receiver().is_some() {
            // `Rails.application.routes.draw` gets re-found as a nested
            // call when we walk a weird input; skip anything with an
            // explicit receiver here.
            continue;
        }
        let method = constant_id_str(&call.name()).to_string();

        // Block-wrapping DSLs we passthrough by flattening their
        // block contents into the outer entry list:
        //
        //   - `constraints :id => /regex/ do …` — restricts URL
        //     param matching; the route still resolves to the same
        //     controller#action.
        //   - `member do …` / `collection do …` (Rails resource-
        //     scoping wrappers) — these DO change the id segment the
        //     flattener prepends (`/resource/:id/reply` for member,
        //     `/resource/search` for collection, vs the bare-verb
        //     default `/resource/:resource_id/…`), so we tag each
        //     flattened child with its `ResourceScope` and let the
        //     flattener build the right path. `find_comment` reading
        //     `params[:id]` depends on the member routes carrying `:id`.
        //   - `authenticate` / `authenticated` / `unauthenticated` /
        //     `devise_scope` — Devise visibility wrappers. Runtime auth
        //     is not modeled; nested routes still belong in the table.
        if matches!(
            method.as_str(),
            "constraints"
                | "member"
                | "collection"
                | "authenticate"
                | "authenticated"
                | "unauthenticated"
                | "devise_scope"
        ) {
            if let Some(block_node) = call.block() {
                if let Some(block) = block_node.as_block_node() {
                    if let Some(inner_body) = block.body() {
                        let mut inner = cx.with_mount_scope(cx.mount_scope_depth.get() + 1, || {
                            ingest_route_body(inner_body, file, parent, cx)
                        })?;
                        let scope = match method.as_str() {
                            "member" => Some(ResourceScope::Member),
                            "collection" => Some(ResourceScope::Collection),
                            _ => None, // constraints / Devise wrappers: no scope change
                        };
                        if let Some(scope) = scope {
                            retag_scope(&mut inner, scope);
                        }
                        entries.extend(inner);
                    }
                }
            }
            continue;
        }

        // Per-entry recovery: one `mount`/`use_doorkeeper` must not
        // zero the whole table. Survey mode records the gap and keeps
        // walking; strict mode still fails loud.
        match ingest_route_call(&call, &method, file, parent, cx) {
            Ok(Some(spec)) if method == "match" => match expand_match_via(&call, spec, file) {
                Ok(expanded) => entries.extend(expanded),
                Err(err) if super::survey::is_active() => super::survey::record(&err),
                Err(err) => return Err(err),
            },
            Ok(Some(spec)) => entries.push(spec),
            Ok(None) => {}
            Err(err) if super::survey::is_active() => super::survey::record(&err),
            Err(err) => return Err(err),
        }
    }
    Ok(entries)
}

/// Apply a `member do`/`collection do` scope to every explicit route in
/// `entries`, looking through the facet-less scopes a wrapper (`routing_method
/// :x do`, `concerns`, a conditional's arm) leaves in between.
fn retag_scope(entries: &mut [RouteSpec], scope: ResourceScope) {
    for entry in entries {
        match entry {
            RouteSpec::Explicit { scope: s, .. } => *s = scope,
            RouteSpec::Scope { path: None, module: None, as_prefix: None, entries, .. } => {
                retag_scope(entries, scope)
            }
            _ => {}
        }
    }
}

/// Redirect routes collected during the entry walk.
///
/// A thread-local sink rather than an accumulator threaded through
/// nine recursive signatures, and rather than the second walk
/// `direct_helpers` uses: the action name has to be unique across the
/// whole table, and only the walk that sees every route in order can
/// say that. Same shape as `survey`'s collector, same reason.
mod redirect_sink {
    use std::cell::RefCell;

    use crate::dialect::RedirectRoute;
    use crate::ident::Symbol;

    thread_local! {
        static SINK: RefCell<Vec<RedirectRoute>> = const { RefCell::new(Vec::new()) };
    }

    /// Record one redirect and answer the action name synthesized for
    /// it: the path, made into an identifier, with a counter appended
    /// if an earlier route already took that name.
    pub(super) fn push(path: &str, location: String, status: u16) -> Symbol {
        push_with(path, location, status, false, false)
    }

    pub(super) fn push_keeping_query(path: &str, location: String, status: u16) -> Symbol {
        push_with(path, location, status, false, true)
    }

    fn push_with(
        path: &str,
        location: String,
        status: u16,
        location_is_expression: bool,
        keep_query: bool,
    ) -> Symbol {
        SINK.with(|sink| {
            let mut sink = sink.borrow_mut();
            let base = action_name(path);
            let mut name = base.clone();
            let mut n = 1;
            while sink.iter().any(|r| r.action.as_str() == name) {
                n += 1;
                name = format!("{base}_{n}");
            }
            let action = Symbol::from(name.as_str());
            sink.push(RedirectRoute {
                action: action.clone(),
                location,
                status,
                location_is_expression,
                keep_query,
            });
            action
        })
    }

    pub(super) fn drain() -> Vec<RedirectRoute> {
        SINK.with(|sink| std::mem::take(&mut *sink.borrow_mut()))
    }

    /// `/` → `root`, `/admin` → `admin`, `/a/b` → `a_b`, `/x/:id` →
    /// `x_id`. A leading digit cannot start a method name, so it gets a
    /// prefix rather than being dropped.
    fn action_name(path: &str) -> String {
        let mut name: String = path
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        name = name.trim_matches('_').to_string();
        while name.contains("__") {
            name = name.replace("__", "_");
        }
        if name.is_empty() {
            return "root".to_string();
        }
        if name.starts_with(|c: char| c.is_ascii_digit()) {
            return format!("redirect_{name}");
        }
        name
    }
}

/// The controller the synthesized redirect actions live on. Named for
/// what it is so an emitted tree reads honestly; an app that happens to
/// define this class would collide, which is why the name is one no
/// generator produces.
pub const REDIRECT_CONTROLLER: &str = "RoundhouseRedirectsController";

/// Rails' own health-check controller (`get "up" => "rails/health#show"`
/// in every `rails new` app). Ingest synthesizes it when a route
/// targets it and the app defines none; see
/// `project::emits_namespaced_controllers` for the targets that receive it.
pub const RAILS_HEALTH_CONTROLLER: &str = "Rails::HealthController";

/// `redirect("/path")` / `redirect("/path", status: 302)` — the literal
/// form, which is all that can be served without running Rails'
/// redirect block. Answers the location and the status Rails would use.
fn redirect_literal(node: &Node<'_>) -> Option<(String, u16, bool)> {
    let call = node.as_call_node()?;
    if call.receiver().is_some() {
        return None;
    }
    let name = call.name();
    if constant_id_str(&name) != "redirect" {
        return None;
    }
    if let Some(block) = call.block().and_then(|block| block.as_block_node()) {
        let (location, status) = redirect_block(block, redirect_status_from_call(&call))?;
        return Some((location, status, false));
    }
    let Some(arguments) = call.arguments() else { return None };
    let mut location = None;
    let mut status = 301;
    let mut path_option = false;
    for argument in arguments.arguments().iter() {
        if let Some(s) = string_value(&argument) {
            location.get_or_insert(s);
            continue;
        }
        let Some(hash) = argument.as_keyword_hash_node() else { return None };
        for element in hash.elements().iter() {
            let Some(assoc) = element.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            match key.as_str() {
                "status" => {
                    let value = assoc.value();
                    let code = value
                        .as_integer_node()
                        .and_then(|i| super::util::integer_i64(&i.value()))
                        .and_then(|i| u16::try_from(i).ok())?;
                    status = code;
                }
                // `redirect(path: "/login")` is Rails' options form of a
                // path-only redirect: the same location a positional
                // string carries, without host, protocol, or query.
                // Other options (`subdomain:`, `host:`) rebuild the
                // request URL and stay unmodeled.
                "path" => {
                    // Distinct from a positional string: the caller marks
                    // this route so the synthesized action keeps the
                    // request query string. Rails' options hash wins
                    // over a positional string, so `redirect("/old",
                    // path: "/new")` goes to `/new`, not `/old`.
                    path_option = true;
                    location = Some(string_value(&assoc.value())?);
                }
                _ => return None,
            }
        }
    }
    Some((location?, status, path_option))
}

/// `redirect { |params, request| "/path" }` when the block returns a
/// string. One or two block parameters are accepted. A block that does
/// not return a string stays unsupported.
fn redirect_block(block: ruby_prism::BlockNode<'_>, status: u16) -> Option<(String, u16)> {
    let params = block.parameters().and_then(|params| params.as_block_parameters_node());
    let names = params
        .and_then(|params| params.parameters())
        .map(|list| {
            list.requireds()
                .iter()
                .filter_map(|param| param.as_required_parameter_node())
                .map(|param| constant_id_str(&param.name()).to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.len() > 2 || names.iter().any(|name| name != "_" && name != "params" && name != "request" && name != "req") {
        return None;
    }
    let body = block.body()?;
    let source = super::expr::ingest_expr(&body, "<redirect>").ok()?;
    if !redirect_expression_is_string(&source) {
        return None;
    }
    let mut rendered = crate::emit::ruby::emit_expr(&source);
    // The synthesized action reads the request as `request`. A block
    // parameter named `req` is the same object.
    rendered = rendered.replace("req.", "request.");
    Some((format!("\u{0}{rendered}"), status))
}

fn redirect_status_from_call(call: &ruby_prism::CallNode<'_>) -> u16 {
    let Some(arguments) = call.arguments() else { return 301 };
    for argument in arguments.arguments().iter() {
        let Some(hash) = argument.as_keyword_hash_node() else { continue };
        for element in hash.elements().iter() {
            let Some(assoc) = element.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            if key.as_str() != "status" {
                continue;
            }
            if let Some(code) = assoc
                .value()
                .as_integer_node()
                .and_then(|i| super::util::integer_i64(&i.value()))
                .and_then(|i| u16::try_from(i).ok())
            {
                return code;
            }
        }
    }
    301
}

fn redirect_expression_is_string(expr: &crate::expr::Expr) -> bool {
    match &*expr.node {
        crate::expr::ExprNode::Lit { value: crate::expr::Literal::Str { .. } } => true,
        crate::expr::ExprNode::StringInterp { .. } => true,
        crate::expr::ExprNode::If { then_branch, else_branch, .. } => {
            redirect_expression_is_string(then_branch) && redirect_expression_is_string(else_branch)
        }
        crate::expr::ExprNode::Send { method, recv, args, .. } => {
            // `present?` is rewritten to `!(...).strip.empty?` before this
            // check. The result is a string when that call's argument is.
            if method.as_str() == "!" {
                return args.iter().any(redirect_expression_is_string);
            }
            if matches!(method.as_str(), "strip" | "empty?") {
                return recv.as_ref().is_some_and(redirect_expression_is_string)
                    || args.iter().any(redirect_expression_is_string);
            }
            matches!(
                method.as_str(),
                "query_string" | "path" | "fullpath" | "to_s" | "+" | "[]" | "present?"
            )
        }
        crate::expr::ExprNode::Seq { exprs } => exprs
            .last()
            .is_some_and(redirect_expression_is_string),
        _ => false,
    }
}

fn ingest_route_call(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Option<RouteSpec>> {
    // Verb shortcuts (`get "/p", to: "c#a"` and the hashrocket form
    // `get "/p" => "c#a"`). `ingest_explicit_route` returns Ok(None)
    // for shapes it intentionally drops (today: `to: redirect(...)`
    // helpers — not bench-critical, not modeled in `RouteSpec`).
    if let Some(http) = http_method_from(method) {
        // A `match` takes its verbs from `via:` in `expand_match_via`,
        // once the entry is built; expanding it here too would copy
        // each of these routes again per verb.
        let via = if method == "match" { Vec::new() } else { via_methods(call) };
        if via.is_empty() {
            return ingest_explicit_route(call, http, file, parent);
        }
        let mut entries = Vec::new();
        let mut dropped = false;
        for method in via {
            // A dropped target is the same for every verb. Do not ingest
            // it again: that repeated the survey line and, before the
            // empty check, panicked.
            if dropped {
                continue;
            }
            if let Some(route) = ingest_explicit_route(call, method, file, parent)? {
                entries.push(route);
            } else {
                dropped = true;
            }
        }
        let Some(first) = entries.pop() else { return Ok(None) };
        if entries.is_empty() {
            return Ok(Some(first));
        }
        entries.insert(0, first);
        return Ok(Some(entries.into_iter().reduce(|left, right| match (left, right) {
            (RouteSpec::Scope { mut entries, .. }, route) => {
                entries.push(route);
                RouteSpec::Scope { path: None, module: None, as_prefix: None, defaults: IndexMap::new(), nest: false, entries }
            }
            (left, right) => RouteSpec::Scope { path: None, module: None, as_prefix: None, defaults: IndexMap::new(), nest: false, entries: vec![left, right] },
        }).expect("via produced a route")));
    }
    match method {
        "root" => ingest_root_route(call, file),
        "resources" => ingest_resources_route(call, file, cx, false).map(Some),
        "resource" => ingest_resources_route(call, file, cx, true).map(Some),
        "namespace" => ingest_namespace_route(call, file, cx).map(Some),
        "scope" => ingest_scope_route(call, file, cx).map(Some),
        // `nested do … end` — the explicit form of the nesting a
        // `resources` block already applies to a child `resources` or
        // verb call. It carries no facets of its own; what it does is
        // force the ENCLOSING resource's `/parent/:parent_id` prefix to
        // be materialized before anything inside runs, which is the
        // only way a `scope path:` lands inside it rather than in front
        // of it (`scope` is not one of the calls Rails auto-nests).
        "nested" => Ok(Some(RouteSpec::Scope {
            path: None,
            module: None,
            as_prefix: None,
            defaults: IndexMap::new(),
            nest: true,
            entries: block_entries(call, file, None, cx)?,
        })),
        "draw" | "load" | "instance_eval" => ingest_route_file_include(call, &method, file, cx),
        // `concern :name do |options| … end` defines a macro and adds no
        // route; `concerns :name, k: v` runs it here.
        "concern" => {
            register_concern(call, file, cx);
            Ok(None)
        }
        "concerns" => ingest_concerns(call, file, parent, cx),
        // `resolve("Model") { |m| route_for(…) }` teaches `polymorphic_url`
        // how to build a URL for a model. It names no route and defines
        // no helper, so there is nothing for the table to hold.
        "resolve" => Ok(None),
        // Keep supported siblings and the ordinary error stream. A mount
        // is not a parse failure, and --allow-unsupported can inspect the
        // incomplete output. Only the existing fixed runtime cable endpoint
        // is exempt; this does not add engine route composition.
        "mount" => {
            if cx.mount_scope_depth.get() == 0 && runtime_cable_mount(call) {
                return Ok(None);
            }
            let location = call.location();
            let detail = "mounted Rack applications and engine routes are not composed into the host route table";
            cx.diagnostics.borrow_mut().push(Diagnostic::unsupported(
                Span {
                    file: super::sources::file_id(file),
                    start: location.start_offset() as u32,
                    end: location.end_offset() as u32,
                },
                None,
                "route mount",
                detail,
            ));
            super::survey::record(&IngestError::Unsupported {
                file: file.into(),
                message: format!("route mount: {detail}"),
            });
            Ok(None)
        }
        // `direct :fresh_user_avatar do |user, options| … end` — a
        // custom URL helper, not a route: it adds no path to the table,
        // it names a `<name>_path`/`_url` builder whose body is
        // arbitrary Ruby. No `RouteSpec` variant can hold that, and
        // generating the helper needs both a typed signature for the
        // block's params and query-string support in the emitted
        // helpers (`route_for :user_avatar, token, v: …` →
        // "/users/…/avatar?v=…"), which the segment-interpolation
        // builder has no notion of. Dropped here with the helper NAME
        // in the ledger line, so the hole reads as "`x_path` is
        // missing" rather than "some DSL was skipped".
        // Collected out-of-band by `collect_direct_helpers` — it is a
        // custom URL helper, not a route, so it contributes no entry
        // here.
        "direct" => Ok(None),
        // `devise_for` — static Devise route/helper table; see
        // `ingest::devise_routes`. Controllers override via
        // `controllers:`. Does not model Warden or OmniAuth callbacks.
        "devise_for" => super::devise_routes::ingest_devise_for(call, file),
        // A block-taking method the app added to the mapper
        // (`routing_method :x do … end`): a scope over the enclosing
        // mapper that changes no path.
        _ if cx.wrappers.contains(method) => Ok(Some(RouteSpec::Scope {
            path: None,
            module: None,
            as_prefix: None,
            defaults: IndexMap::new(),
            nest: false,
            entries: block_entries(call, file, parent, cx)?,
        })),
        // Unknown DSL — `concern`, `use_doorkeeper`, `authenticate`
        // (non-block), etc. land here. Strict ingest fails loud so the
        // fixture that introduces them forces a recognizer; survey
        // callers get a per-entry ledger line (see ingest_route_stmts).
        _ => Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported routes DSL: `{method}`"),
        }),
    }
}

fn via_methods(call: &ruby_prism::CallNode<'_>) -> Vec<HttpMethod> {
    let Some(args) = call.arguments() else { return Vec::new() };
    for arg in args.arguments().iter() {
        let Some(hash) = arg.as_keyword_hash_node() else { continue };
        for element in hash.elements().iter() {
            let Some(assoc) = element.as_assoc_node() else { continue };
            if symbol_value(&assoc.key()).as_deref() != Some("via") {
                continue;
            }
            let values = assoc.value().as_array_node().map(|array| array.elements().iter().collect()).unwrap_or_else(|| vec![assoc.value()]);
            return values.iter().filter_map(|value| symbol_value(value).as_deref().and_then(http_method_from)).collect();
        }
    }
    Vec::new()
}

/// `match "x", to: "c#a", via: %i[get post]` is one route per verb in
/// Rails (`GET|POST /x`); the entry ingests once with the `Any`
/// placeholder and is copied here with each listed verb. Only
/// `via: :all` keeps `Any`, the verb the runtime router matches
/// against every request method. A verb the table cannot represent
/// (`via: :trace`), a `via:` that is not a literal, or no `via:` at all
/// (which Rails refuses) is unsupported rather than widened to `Any`.
fn expand_match_via(
    call: &ruby_prism::CallNode<'_>,
    spec: RouteSpec,
    file: &str,
) -> Result<Vec<RouteSpec>, IngestError> {
    let unsupported = |message: String| IngestError::Unsupported { file: file.into(), message };
    let names = match_via_names(call).map_err(unsupported)?;
    let mut verbs = Vec::with_capacity(names.len());
    for name in &names {
        let verb = match name.as_str() {
            "all" => HttpMethod::Any,
            other => http_method_from(other)
                .filter(|m| *m != HttpMethod::Any)
                .ok_or_else(|| unsupported(format!("unsupported `match` verb: `via: :{other}`")))?,
        };
        verbs.push(verb);
    }
    if verbs.contains(&HttpMethod::Any) {
        return Ok(vec![spec]);
    }
    Ok(verbs
        .into_iter()
        .map(|verb| {
            let mut entry = spec.clone();
            if let RouteSpec::Explicit { method, .. } = &mut entry {
                *method = verb;
            }
            entry
        })
        .collect())
}

/// The `via:` value of a call as written: `:get`, `"post"`, or a
/// literal list of either, lowercased. An error when `via:` is absent,
/// empty, or anything but symbol/string literals.
fn match_via_names(call: &ruby_prism::CallNode<'_>) -> Result<Vec<String>, String> {
    // Ruby keeps the last of duplicate keys, so `via: :get, via: :post`
    // is `via: :post`.
    let via = call.arguments().and_then(|args| {
        args.arguments()
            .iter()
            .filter_map(|arg| arg.as_keyword_hash_node())
            .flat_map(|kh| kh.elements().iter().collect::<Vec<_>>())
            .filter_map(|el| el.as_assoc_node())
            .filter(|assoc| symbol_value(&assoc.key()).as_deref() == Some("via"))
            .map(|assoc| assoc.value())
            .last()
    });
    let Some(via) = via else {
        return Err("`match` without `via:` (Rails requires the verbs)".into());
    };
    let names: Vec<String> = match via.as_array_node() {
        Some(arr) => arr
            .elements()
            .iter()
            .map(|n| via_name(&n))
            .collect::<Result<_, _>>()?,
        None => vec![via_name(&via)?],
    };
    if names.is_empty() {
        return Err("unsupported `match` option: non-literal `via:`".into());
    }
    Ok(names)
}

/// One `via:` element, lowercased. Rails upcases any other spelling of
/// a verb (`:GET`, `"post"` both work), but only the exact symbol
/// `:all` means every verb: `"all"` and `:ALL` become a literal `ALL`
/// request method that no request carries, so those are unsupported
/// rather than widened to `Any`.
fn via_name(node: &Node<'_>) -> Result<String, String> {
    if symbol_value(node).as_deref() == Some("all") {
        return Ok("all".into());
    }
    let name = symbol_or_string_value(node)
        .ok_or_else(|| "unsupported `match` option: non-literal `via:`".to_string())?;
    if name.eq_ignore_ascii_case("all") {
        let spelled = if symbol_value(node).is_some() {
            format!(":{name}")
        } else {
            format!("{name:?}")
        };
        return Err(format!(
            "unsupported `match` verb: `via: {spelled}` (only the symbol `:all` means every verb)"
        ));
    }
    Ok(name.to_lowercase())
}

fn http_method_from(name: &str) -> Option<HttpMethod> {
    Some(match name {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "patch" => HttpMethod::Patch,
        "delete" => HttpMethod::Delete,
        "head" => HttpMethod::Head,
        "options" => HttpMethod::Options,
        "match" => HttpMethod::Any,
        _ => return None,
    })
}

/// First positional symbol-or-string argument (`namespace :admin`,
/// `scope "v2"`, `draw(:api)`).
fn first_name_arg(call: &ruby_prism::CallNode<'_>) -> Option<String> {
    let args = call.arguments()?;
    for arg in args.arguments().iter() {
        if let Some(s) = rsymbol(&arg) {
            return Some(s);
        }
        if let Some(s) = rstring(&arg) {
            return Some(s);
        }
        // Keyword hash → options-only call (`scope module: :web`).
        if arg.as_keyword_hash_node().is_some() {
            return None;
        }
    }
    None
}

fn block_entries(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    cx.with_mount_scope(cx.mount_scope_depth.get() + 1, || match call.block() {
        Some(block_node) => match block_node.as_block_node() {
            Some(block) => match block.body() {
                Some(body) => ingest_route_body(body, file, parent, cx),
                None => Ok(Vec::new()),
            },
            None => Ok(Vec::new()),
        },
        None => Ok(Vec::new()),
    })
}

/// `namespace :admin do … end` — `scope` with path, controller module,
/// and helper prefix all set to the name. Resets the enclosing
/// `resources` inference context (Rails does not infer member
/// controllers across a namespace boundary).
fn ingest_namespace_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    cx: &Ctx<'_>,
) -> IngestResult<RouteSpec> {
    let Some(name) = first_name_arg(call) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "namespace without a name".into(),
        });
    };
    let entries = block_entries(call, file, None, cx)?;
    Ok(RouteSpec::Scope {
        path: Some(name.clone()),
        module: Some(name.clone()),
        as_prefix: Some(name),
        defaults: IndexMap::new(),
        nest: false,
        entries,
    })
}

/// `scope <path> [, path:, module:, as:] do … end` — each facet
/// independent. A positional symbol/string is the path segment
/// (`scope :v1_alpha, as: :v1_alpha, module: :v1`).
fn ingest_scope_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    cx: &Ctx<'_>,
) -> IngestResult<RouteSpec> {
    let mut path = first_name_arg(call);
    let mut module: Option<String> = None;
    let mut as_prefix: Option<String> = None;
    let mut defaults: IndexMap<Symbol, String> = IndexMap::new();
    if let Some(args) = call.arguments() {
        for arg in args.arguments().iter() {
            let Some(kh) = arg.as_keyword_hash_node() else { continue };
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = assoc.value();
                let val = rname(&value);
                match key.as_str() {
                    "path" => path = val.or(path),
                    "module" => module = val,
                    "as" => as_prefix = val,
                    // `defaults: { user_id: "me" }` fills a dynamic
                    // segment the caller omits, which makes the
                    // generated helper's parameter OPTIONAL — this used
                    // to be dropped as "shapes the request, not the
                    // (path, controller, action) triple", and that is
                    // true of the triple but not of the signature.
                    // campfire calls `user_profile_url` with no argument.
                    "defaults" => {
                        if let Some(h) = value.as_hash_node() {
                            for el in h.elements().iter() {
                                let Some(a) = el.as_assoc_node() else { continue };
                                let Some(k) = symbol_value(&a.key()) else { continue };
                                let Some(v) = string_value(&a.value())
                                    .or_else(|| symbol_value(&a.value()))
                                else {
                                    continue;
                                };
                                defaults.insert(Symbol::from(k.as_str()), v);
                            }
                        }
                    }
                    // `constraints:` / `format:` shape the request, not
                    // the (path, controller, action) triple.
                    _ => {}
                }
            }
        }
    }
    let entries = block_entries(call, file, None, cx)?;
    Ok(RouteSpec::Scope { path, module, as_prefix, defaults, nest: false, entries })
}

/// `draw(:admin)` / `load(root.join("config", "routes", "admin.rb"))` /
/// `instance_eval(File.read(root.join("config/routes/admin.rb")))` —
/// Rails runs another route file. All three name a file under
/// `config/routes/`, and the ingester keys those by their path below
/// that directory (`admin`, `services/internal`).
///
/// `draw(:name)` runs the file inside the CURRENT mapper, so its
/// statements are route DSL directly and inherit the enclosing scope:
/// included entries ride a facet-less Scope the flattener composes
/// transparently. `load` runs the file as plain Ruby, and a routes file
/// written for it carries its own `Rails.application.routes.draw do …
/// end` — a fresh mapper, so its routes belong at the top of the table
/// wherever the `load` was written.
fn ingest_route_file_include(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
    file: &str,
    cx: &Ctx<'_>,
) -> IngestResult<Option<RouteSpec>> {
    let Some(args) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("{method} without a route-file name"),
        });
    };
    let first = args.arguments().iter().next();
    let key = first.as_ref().and_then(|arg| {
        if method == "draw" {
            // `draw(:admin)`; a path-shaped argument also names a file.
            symbol_or_string_value(arg).or_else(|| route_file_key(arg))
        } else {
            route_file_key(arg)
        }
    });
    let Some(key) = key else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("{method} of a route file whose path is not a literal"),
        });
    };
    let Some((source, path)) = resolve_draw_name(&key, cx.draws) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("{method}(:{key}) — config/routes/{key}.rb not found"),
        });
    };
    if cx.active.borrow().iter().any(|k| k == &key) {
        return Ok(None);
    }
    super::sources::register(path, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, path);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return Err(IngestError::Parse {
            file: path.clone(),
            message: "route file is not a program".into(),
        });
    };
    cx.active.borrow_mut().push(key);
    let included = (|| -> IngestResult<Option<RouteSpec>> {
        let drawn = top_level_app_draws(&root);
        if method != "draw" && !drawn.is_empty() {
            let entries = cx.with_mount_scope(0, || -> IngestResult<Vec<RouteSpec>> {
                let mut entries = Vec::new();
                for draw_call in &drawn {
                    let Some(block) = draw_call.block().and_then(|b| b.as_block_node()) else {
                        continue;
                    };
                    if let Some(body) = block.body() {
                        entries.extend(ingest_route_body(body, path, None, cx)?);
                    }
                }
                Ok(entries)
            })?;
            cx.hoisted.borrow_mut().extend(entries);
            return Ok(None);
        }
        let entries =
            ingest_route_stmts(program.statements().body().iter(), path, None, cx)?;
        Ok(Some(RouteSpec::Scope {
            path: None,
            module: None,
            as_prefix: None,
            defaults: IndexMap::new(),
            nest: false,
            entries,
        }))
    })();
    cx.active.borrow_mut().pop();
    included
}

/// The route-file key a path expression names: the literal fragments of
/// `root.join("config", "routes", "services", "internal.rb").to_s`,
/// `Rails.root.join("config/routes/x.rb")`, `File.read(…)` of either,
/// read below the last `config/routes/` and without the extension.
fn route_file_key(node: &Node<'_>) -> Option<String> {
    let mut parts = Vec::new();
    path_fragments(node, &mut parts);
    let joined = parts.join("/");
    let below = joined.rsplit_once("config/routes/")?.1;
    let key = below.strip_suffix(".rb").unwrap_or(below);
    (!key.is_empty()).then(|| key.to_string())
}

fn path_fragments(node: &Node<'_>, out: &mut Vec<String>) {
    if let Some(s) = string_value(node) {
        out.push(s.trim_matches('/').to_string());
    } else if let Some(sym) = symbol_value(node) {
        out.push(sym);
    } else if let Some(call) = node.as_call_node() {
        if let Some(recv) = call.receiver() {
            path_fragments(&recv, out);
        }
        if let Some(args) = call.arguments() {
            for arg in args.arguments().iter() {
                path_fragments(&arg, out);
            }
        }
    } else if let Some(parens) = node.as_parentheses_node() {
        if let Some(body) = parens.body() {
            for stmt in flatten_statements(body) {
                path_fragments(&stmt, out);
            }
        }
    }
}

/// `concern :graphql_routes do |options| … end`.
fn register_concern(call: &ruby_prism::CallNode<'_>, file: &str, cx: &Ctx<'_>) {
    let Some(name) = call
        .arguments()
        .and_then(|args| args.arguments().iter().next())
        .and_then(|arg| symbol_or_string_value(&arg))
    else {
        return;
    };
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return };
    let param = block
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|p| p.parameters())
        .and_then(|p| {
            p.requireds()
                .iter()
                .next()
                .and_then(|r| r.as_required_parameter_node().map(|r| constant_id_str(&r.name()).to_string()))
                .or_else(|| {
                    p.optionals().iter().next().and_then(|o| {
                        o.as_optional_parameter_node()
                            .map(|o| constant_id_str(&o.name()).to_string())
                    })
                })
        });
    let body = block.body().map(|b| {
        let location = b.location();
        // Reparse only the macro body, retaining its original byte offsets so
        // a recovered mount diagnostic still points into the defining file.
        let mut source = vec![b' '; location.start_offset()];
        source.extend_from_slice(location.as_slice());
        source
    });
    cx.concerns
        .borrow_mut()
        .insert(name, Concern { param, body, file: file.to_string() });
}

/// `concerns :a, :b, k: v` — run each named concern here, its options
/// bound to the block's parameter. Rails builds a fresh options hash per
/// concern from the trailing keywords.
fn ingest_concerns(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Option<RouteSpec>> {
    let Some(args) = call.arguments() else { return Ok(None) };
    let mut names = Vec::new();
    let mut options = eval::RVal::Hash(Vec::new());
    for arg in args.arguments().iter() {
        if let Some(name) = symbol_or_string_value(&arg) {
            names.push(name);
        } else if arg.as_keyword_hash_node().is_some() || arg.as_hash_node().is_some() {
            options = eval::value(&arg).unwrap_or(eval::RVal::Hash(Vec::new()));
        }
    }
    let entries = run_concerns(&names, &options, file, parent, cx)?;
    Ok(Some(RouteSpec::Scope {
        path: None,
        module: None,
        as_prefix: None,
        defaults: IndexMap::new(),
        nest: false,
        entries,
    }))
}

/// Replay each named concern's body where it is used, `options` bound to
/// the block's parameter. Shared by `concerns :a, :b` and the
/// `resources :x, concerns: :a` option, which Rails routes through the
/// same `concerns` call inside the resource's scope.
fn run_concerns(
    names: &[String],
    options: &eval::RVal,
    file: &str,
    parent: Option<&str>,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    let mut entries = Vec::new();
    for name in names {
        let (param, body, def_file) = {
            let concerns = cx.concerns.borrow();
            let Some(concern) = concerns.get(name) else {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: format!("concerns :{name} — no such concern"),
                });
            };
            (concern.param.clone(), concern.body.clone(), concern.file.clone())
        };
        let Some(body) = body else { continue };
        if cx.active.borrow().iter().any(|k| k == &format!("concern:{name}")) {
            continue;
        }
        let result = super::prism::parse(&body, &def_file);
        let root = result.node();
        let Some(program) = root.as_program_node() else { continue };
        eval::push_frame();
        if let Some(param) = &param {
            eval::bind(param, options.clone());
        }
        cx.active.borrow_mut().push(format!("concern:{name}"));
        let ran = ingest_route_stmts(program.statements().body().iter(), &def_file, parent, cx);
        cx.active.borrow_mut().pop();
        eval::pop_frame();
        entries.extend(ran?);
    }
    Ok(entries)
}

/// The values a route file computes with — the small slice of Ruby a
/// `concern` needs (`options[:prefix] || ""`, `"#{prefix}x"`), evaluated
/// where the DSL reads a name or a path. Anything else is unknown, and
/// an unknown name drops its route exactly as before.
mod eval {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use ruby_prism::Node;

    use crate::ingest::util::{constant_id_str, flatten_statements};

    #[derive(Clone, Debug)]
    pub(super) enum RVal {
        Str(String),
        Sym(String),
        Nil,
        Hash(Vec<(String, RVal)>),
        Unknown,
    }

    thread_local! {
        static FRAMES: RefCell<Vec<HashMap<String, RVal>>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn push_frame() {
        FRAMES.with(|f| f.borrow_mut().push(HashMap::new()));
    }

    pub(super) fn pop_frame() {
        FRAMES.with(|f| {
            f.borrow_mut().pop();
        });
    }

    pub(super) fn bind(name: &str, value: RVal) {
        FRAMES.with(|f| {
            if let Some(top) = f.borrow_mut().last_mut() {
                top.insert(name.to_string(), value);
            }
        });
    }

    fn lookup(name: &str) -> Option<RVal> {
        FRAMES.with(|f| f.borrow().last().and_then(|top| top.get(name).cloned()))
    }

    fn text(v: &RVal) -> Option<String> {
        match v {
            RVal::Str(s) | RVal::Sym(s) => Some(s.clone()),
            RVal::Nil => Some(String::new()),
            _ => None,
        }
    }

    /// Evaluate `node`; `None` for anything outside the slice.
    pub(super) fn value(node: &Node<'_>) -> Option<RVal> {
        if let Some(s) = node.as_string_node() {
            return Some(RVal::Str(String::from_utf8_lossy(s.unescaped()).into_owned()));
        }
        if let Some(s) = node.as_symbol_node() {
            let loc = s.value_loc()?;
            return Some(RVal::Sym(String::from_utf8_lossy(loc.as_slice()).into_owned()));
        }
        if node.as_nil_node().is_some() {
            return Some(RVal::Nil);
        }
        if let Some(read) = node.as_local_variable_read_node() {
            return lookup(constant_id_str(&read.name()));
        }
        if let Some(interp) = node.as_interpolated_string_node() {
            return interpolate(interp.parts().iter()).map(RVal::Str);
        }
        if let Some(interp) = node.as_interpolated_symbol_node() {
            return interpolate(interp.parts().iter()).map(RVal::Sym);
        }
        if let Some(or) = node.as_or_node() {
            let left = value(&or.left())?;
            return match left {
                RVal::Nil => value(&or.right()),
                other => Some(other),
            };
        }
        if let Some(parens) = node.as_parentheses_node() {
            let body = parens.body()?;
            let stmts = flatten_statements(body);
            return match stmts.as_slice() {
                [only] => value(only),
                _ => None,
            };
        }
        if let Some(call) = node.as_call_node() {
            // Concern bodies are reparsed without the block-parameter
            // declaration, so a bound parameter like `options` parses as a
            // receiverless CallNode. Resolve it through the bound frame.
            if call.receiver().is_none() && call.arguments().is_none() && call.block().is_none() {
                if let Some(bound) = lookup(constant_id_str(&call.name())) {
                    return Some(bound);
                }
            }
            // `options[:key]`
            if constant_id_str(&call.name()) == "[]" {
                let recv = value(&call.receiver()?)?;
                let args = call.arguments()?;
                let key = value(&args.arguments().iter().next()?)?;
                let key = text(&key)?;
                return match recv {
                    RVal::Hash(pairs) => Some(
                        pairs
                            .into_iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, v)| v)
                            .unwrap_or(RVal::Nil),
                    ),
                    RVal::Nil => Some(RVal::Nil),
                    _ => None,
                };
            }
            return None;
        }
        if let Some(hash) = node.as_keyword_hash_node() {
            return pairs(hash.elements().iter());
        }
        if let Some(hash) = node.as_hash_node() {
            return pairs(hash.elements().iter());
        }
        None
    }

    fn pairs<'pr>(elements: impl Iterator<Item = Node<'pr>>) -> Option<RVal> {
        let mut out = Vec::new();
        for el in elements {
            let assoc = el.as_assoc_node()?;
            let key = text(&value(&assoc.key())?)?;
            out.push((key, value(&assoc.value()).unwrap_or(RVal::Unknown)));
        }
        Some(RVal::Hash(out))
    }

    fn interpolate<'pr>(parts: impl Iterator<Item = Node<'pr>>) -> Option<String> {
        let mut out = String::new();
        for part in parts {
            if let Some(s) = part.as_string_node() {
                out.push_str(&String::from_utf8_lossy(s.unescaped()));
            } else if let Some(embedded) = part.as_embedded_statements_node() {
                let stmts = flatten_statements(embedded.statements()?.as_node());
                let [only] = stmts.as_slice() else { return None };
                out.push_str(&text(&value(only)?)?);
            } else {
                return None;
            }
        }
        Some(out)
    }

    /// A string: a literal, or an expression that evaluates to one.
    pub(super) fn string(node: &Node<'_>) -> Option<String> {
        match value(node)? {
            RVal::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A symbol: a literal, or an expression that evaluates to one.
    pub(super) fn symbol(node: &Node<'_>) -> Option<String> {
        match value(node)? {
            RVal::Sym(s) => Some(s),
            _ => None,
        }
    }
}

/// [`string_value`], plus a string the surrounding concern computed.
fn rstring(node: &Node<'_>) -> Option<String> {
    string_value(node).or_else(|| eval::string(node))
}

/// [`symbol_value`], plus a symbol the surrounding concern computed
/// (`as: :"#{route_prefix}web_pixel_javascript"`).
fn rsymbol(node: &Node<'_>) -> Option<String> {
    symbol_value(node).or_else(|| eval::symbol(node))
}

/// [`symbol_or_string_value`], with the same allowance.
fn rname(node: &Node<'_>) -> Option<String> {
    rsymbol(node).or_else(|| rstring(node))
}

/// Resolve a `draw(:name)` argument against the `config/routes/` file
/// map. `draws` is keyed by the path RELATIVE TO `config/routes/`
/// (without `.rb`), so a subdirectory-nested file's full relative name
/// (`draw('financials/financials_erp_routes')`) matches directly.
/// Falls back to a bare-stem match (the last path segment) when exactly
/// one file under `config/routes/` carries that stem, for backwards
/// compatibility with a `draw(:name)` call that predates any
/// subdirectory nesting; an ambiguous stem (two files share it in
/// different subdirectories) reports not-found rather than guessing.
fn resolve_draw_name<'a>(
    name: &str,
    draws: &'a HashMap<String, (Vec<u8>, String)>,
) -> Option<&'a (Vec<u8>, String)> {
    if let Some(entry) = draws.get(name) {
        return Some(entry);
    }
    let mut matches = draws.iter().filter(|(key, _)| key.rsplit('/').next() == Some(name));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.1)
}

/// Parse and ingest one `config/routes/<name>.rb` split file's
/// top-level statements as route DSL (no `routes.draw` wrapper — Rails
/// loads it straight into the calling `draw` block's context). Shared
/// by a single named `draw(:name)` (`ingest_draw_route`) and by every
/// file a `Dir.glob(...).each { draw(...) }` mass-draw matches
/// (`ingest_glob_draw_each`) — both hand the same (source, path) pair
/// from the `draws` map to the same parse-and-walk.
fn ingest_routes_file_entries(
    source: &[u8],
    path: &str,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    super::sources::register(path, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, path);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return Err(IngestError::Parse {
            file: path.to_string(),
            message: "route file is not a program".into(),
        });
    };
    if cx.active.borrow().iter().any(|active| active == path) {
        return Ok(Vec::new());
    }
    cx.active.borrow_mut().push(path.to_string());
    let result = ingest_route_stmts(program.statements().body().iter(), path, None, cx);
    cx.active.borrow_mut().pop();
    result
}

/// Recognize the receiver of a top-level `<recv>.each do |v| draw(...)
/// end` as `Dir.glob(PATTERN[, base: 'config/routes'])` or
/// `Dir[PATTERN]`, and return PATTERN made relative to
/// `config/routes/` — the same coordinate space `draws`' keys live in.
/// `None` for any other receiver shape, or for a `base:`/prefix that
/// doesn't target `config/routes/` (a glob over some other directory is
/// not this idiom; let it fail loud downstream rather than guess).
fn dir_glob_routes_pattern(recv: &Node<'_>) -> Option<String> {
    let call = recv.as_call_node()?;
    let dir_receiver = call.receiver()?;
    let dir_path = constant_path_of(&dir_receiver)?;
    if dir_path.len() != 1 || dir_path[0] != "Dir" {
        return None;
    }
    let method = constant_id_str(&call.name());
    let args = call.arguments()?;
    if method == "glob" {
        let mut pattern: Option<String> = None;
        let mut base: Option<String> = None;
        for arg in args.arguments().iter() {
            if pattern.is_none() {
                if let Some(s) = string_value(&arg) {
                    pattern = Some(s);
                    continue;
                }
            }
            if let Some(kh) = arg.as_keyword_hash_node() {
                for el in kh.elements().iter() {
                    let Some(assoc) = el.as_assoc_node() else { continue };
                    if symbol_value(&assoc.key()).as_deref() == Some("base") {
                        base = string_value(&assoc.value());
                    }
                }
            }
        }
        let pattern = pattern?;
        return match base {
            Some(b) if b.trim_end_matches('/') == "config/routes" => Some(pattern),
            Some(_) => None,
            None => pattern.strip_prefix("config/routes/").map(|s| s.to_string()),
        };
    }
    if method == "[]" {
        let pattern = args.arguments().iter().next().and_then(|a| string_value(&a))?;
        return pattern.strip_prefix("config/routes/").map(|s| s.to_string());
    }
    None
}

/// `Dir.glob(PATTERN, base: 'config/routes').each do |v| draw(...) end`
/// — draw every `config/routes/` file the glob matches, sorted, each
/// riding its own facet-less Scope exactly like a single named
/// `draw(:name)` does. The block's own argument expression (typically
/// `v.sub(/\.rb$/, '')`, stripping the extension `Dir.glob` includes) is
/// NOT evaluated — the glob match against the VFS already gives the
/// exact file set and their `.rb`-stripped names, which is what that
/// expression is written to reproduce. Only the block SHAPE is
/// checked: a single bare `draw(...)` statement, so a block that does
/// anything else is ledgered by name rather than silently trusted to
/// mean the same thing.
fn ingest_glob_draw_each(
    call: &ruby_prism::CallNode<'_>,
    pattern: &str,
    file: &str,
    cx: &Ctx<'_>,
) -> IngestResult<Vec<RouteSpec>> {
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "unsupported routes DSL: `Dir.glob(...).each` without a block".into(),
        });
    };
    let is_bare_draw = block.body().is_some_and(|body| {
        let stmts = flatten_statements(body);
        stmts.len() == 1
            && stmts[0]
                .as_call_node()
                .is_some_and(|c| c.receiver().is_none() && constant_id_str(&c.name()) == "draw")
    });
    if !is_bare_draw {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "unsupported routes DSL: `Dir.glob(...).each` block body is not a bare `draw(...)` call".into(),
        });
    }

    let mut matched: Vec<&String> =
        cx.draws.keys().filter(|key| glob_matches(pattern, &format!("{key}.rb"))).collect();
    matched.sort();

    let mut entries = Vec::with_capacity(matched.len());
    for key in matched {
        let (source, path) = &cx.draws[key];
        entries.push(RouteSpec::Scope {
            path: None,
            module: None,
            as_prefix: None,
            defaults: IndexMap::new(),
            nest: false,
            entries: ingest_routes_file_entries(source, path, cx)?,
        });
    }
    Ok(entries)
}

/// Match `candidate` (a `/`-separated relative path) against a glob
/// `pattern` supporting only `*` (any run of non-`/` characters, within
/// one path segment) and `**` (zero or more whole path segments) — the
/// two segment kinds `Dir.glob` actually uses in this idiom. No other
/// glob metacharacter (`?`, `{a,b}`, character classes, …) is
/// recognized.
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let cand: Vec<&str> = candidate.split('/').collect();
    glob_match_segments(&pat, &cand)
}

fn glob_match_segments(pat: &[&str], cand: &[&str]) -> bool {
    match pat.first() {
        None => cand.is_empty(),
        Some(&"**") => {
            glob_match_segments(&pat[1..], cand)
                || matches!(cand.split_first(), Some((_, rest)) if glob_match_segments(pat, rest))
        }
        Some(seg) => match cand.split_first() {
            Some((first, rest)) => segment_matches(seg, first) && glob_match_segments(&pat[1..], rest),
            None => false,
        },
    }
}

/// Match one path segment against a single-segment glob pattern whose
/// only metacharacter is `*` (any run of characters — segments are
/// already split on `/`, so it cannot cross a segment boundary).
fn segment_matches(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }
    let Some(rest) = s.strip_prefix(parts[0]) else { return false };
    let mut rest = rest;
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(idx) => rest = &rest[idx + mid.len()..],
            None => return false,
        }
    }
    let last = parts[parts.len() - 1];
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Raw regex-pattern source of a `/.../ ` literal value node — the text
/// between the delimiters, verbatim (escapes like `\/` preserved so it
/// drops straight back into a Ruby regex literal), None for non-regex
/// values. Used for `constraints:` param restrictions (#67).
fn regex_source(node: &Node<'_>) -> Option<String> {
    let r = node.as_regular_expression_node()?;
    Some(String::from_utf8_lossy(r.content_loc().as_slice()).into_owned())
}

fn ingest_explicit_route(
    call: &ruby_prism::CallNode<'_>,
    method: HttpMethod,
    file: &str,
    parent: Option<&str>,
) -> IngestResult<Option<RouteSpec>> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "verb route without arguments".into(),
        });
    };
    let mut path: Option<String> = None;
    let mut to: Option<String> = None;
    let mut to_is_unsupported = false;
    let mut redirect_target: Option<(String, u16, bool)> = None;
    let mut as_name: Option<Symbol> = None;
    let mut action_kwarg: Option<String> = None;
    let mut controller_kwarg: Option<String> = None;
    // The INLINE spelling of `member do … end` / `collection do … end`.
    // Rails accepts both, and campfire writes `delete :clear, on:
    // :collection`; only the block form was recognized, so the route was
    // nested as `/searches/:search_id/clear` (named `search_clear`)
    // where Rails serves `/searches/clear` (named `clear_searches`).
    let mut on_scope: Option<ResourceScope> = None;
    let mut constraints: IndexMap<Symbol, String> = IndexMap::new();

    for arg in args_node.arguments().iter() {
        if let Some(s) = rstring(&arg) {
            // Positional string arg — the path: `get "/p", to: "c#a"`.
            if path.is_none() {
                path = Some(s);
            }
        } else if arg.as_symbol_node().is_some() && path.is_none() && to.is_none() {
            // Positional SYMBOL arg — the same shortcut in the other
            // spelling: `member do get :doff end` is identical to
            // `get "doff"` in Rails, and lobsters' hats routes use it
            // exclusively (`get :doff`, `post :doff_by_user`,
            // `post :update_in_place`, `post :update_by_recreating`).
            // Accepting only the String form silently dropped those four
            // routes and their helpers. Guarded on `to.is_none()` so a
            // symbol appearing after a target can't be mistaken for one.
            if let Some(s) = symbol_value(&arg) {
                path = Some(s);
            }
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            // Two shapes share KeywordHashNode here:
            //   1. Modern kwargs hash: `get "/p", to: "c#a", as: :n` —
            //      path is the prior positional, this hash is all kwargs.
            //   2. Hashrocket-style routing: `get "/p" => "c#a", :as => :n`
            //      — the FIRST entry's key is a String (the path) and
            //      its value is the target string. Subsequent entries
            //      are kwargs (Symbol-keyed).
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let key_node = assoc.key();
                let value = &assoc.value();

                // String-keyed entry → path → target pair (hashrocket
                // form). Only consume the first such entry as path.
                if let Some(key_str) = string_value(&key_node) {
                    if path.is_none() {
                        path = Some(key_str);
                        if let Some(v) = rstring(value) {
                            to = Some(v);
                        } else {
                            // `get "/p" => redirect("/q")` — the
                            // hashrocket spelling of the same literal
                            // redirect the kwarg form takes below.
                            match redirect_literal(value) {
                                Some(r) => redirect_target = Some(r),
                                None => to_is_unsupported = true,
                            }
                        }
                        continue;
                    }
                }

                // Symbol-keyed entry → standard kwarg.
                let Some(key_sym) = symbol_value(&key_node) else { continue };
                match key_sym.as_str() {
                    "to" => {
                        if let Some(v) = rstring(value) {
                            to = Some(v);
                        } else if let Some(r) = redirect_literal(value) {
                            redirect_target = Some(r);
                        } else {
                            // A block redirect (`redirect { |p, req| … }`)
                            // carries no literal to serve, so it stays
                            // dropped — with the ledger line #82 added.
                            to_is_unsupported = true;
                        }
                    }
                    // `:as` accepts either a symbol (`as: :user`) or a
                    // string (`:as => "user"`); lobsters uses the string
                    // form throughout. Without the string fallback the name
                    // was dropped and the helper fell back to the action
                    // name (`show_path` for `user_path`), leaving every
                    // `user_path`/`tag_path`/… call unresolved.
                    "as" => {
                        as_name = rsymbol(value)
                            .map(Symbol::from)
                            .or_else(|| rstring(value).map(Symbol::from));
                    }
                    "on" => {
                        on_scope = match symbol_value(value).as_deref() {
                            Some("member") => Some(ResourceScope::Member),
                            Some("collection") => Some(ResourceScope::Collection),
                            _ => None,
                        };
                    }
                    // `post "suggest", :action => "submit_suggestions"` —
                    // the action override for a resource-scoped shortcut.
                    "action" => action_kwarg = rname(value),
                    // `match :graphql, controller: "one/graphql", action:
                    // :query, via: :post` — the target spelled as two
                    // options where `to: "one/graphql#query"` is one.
                    "controller" => controller_kwarg = rname(value),
                    // `via:` picks the verbs of a `match`; read by
                    // `expand_match_via` once the entry is built. Other
                    // string-value options become routing constraints.
                    "via" => {}
                    // `constraints: { id: /\d+/, tag: /[^,.\/]+/ }` —
                    // per-param regex restrictions. Capture each param's
                    // regex SOURCE (raw text between the delimiters, so
                    // `\/` etc. survive back into a Ruby literal) keyed
                    // by param name. digit-class regexes drive the runtime
                    // router's Integer matcher; the rest let the roda
                    // converter disambiguate two routes that share a
                    // path+verb and differ only by the constraint (#67).
                    "constraints" => {
                        if let Some(h) = value.as_hash_node() {
                            for el in h.elements().iter() {
                                let Some(a) = el.as_assoc_node() else { continue };
                                let Some(param) = symbol_value(&a.key()) else { continue };
                                if let Some(src) = regex_source(&a.value()) {
                                    constraints.insert(Symbol::from(param.as_str()), src);
                                }
                            }
                        }
                    }
                    other => {
                        if let Some(v) = string_value(value) {
                            constraints.insert(Symbol::from(other), v);
                        }
                    }
                }
            }
        }
    }

    if let Some((location, status, keep_query)) = redirect_target {
        // Served by a synthesized action rather than dropped: the app
        // gets the 301 it asked for, and no emitter learns a new route
        // kind for it.
        let path = path.clone().unwrap_or_else(|| "/".to_string());
        let action = if keep_query {
            redirect_sink::push_keeping_query(&path, location, status)
        } else {
            redirect_sink::push(&path, location, status)
        };
        return Ok(Some(RouteSpec::Explicit {
            method,
            path,
            controller: ClassId(Symbol::from(REDIRECT_CONTROLLER)),
            action,
            as_name,
            constraints: IndexMap::new(),
            scope: ResourceScope::default(),
        }));
    }
    if to_is_unsupported {
        // Dropped, with a ledger line: the route is not modeled
        // (`RouteSpec` has no Redirect variant), and a drop nobody can
        // see is how #82's `root to: redirect(...)` went unnoticed.
        // Strict runs still pass — the hole is a missing route, not a
        // miscompile — so this records rather than errors.
        super::survey::record(&IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "route dropped: `{}` with a non-string target (`to: redirect(...)` is not modeled)",
                path.as_deref().unwrap_or("?")
            ),
        });
        return Ok(None);
    }

    let (controller, action) = match to.as_deref().and_then(|s| s.split_once('#')) {
        Some((c, a)) => (c.to_string(), a.to_string()),
        None if controller_kwarg.is_some() => {
            // `controller:` names the class and `action:` the method;
            // without an `action:` Rails takes the path's last segment.
            let Some(p) = path.as_deref() else { return Ok(None) };
            let stem = p.trim_matches('/').rsplit('/').next().unwrap_or("").to_string();
            let action = action_kwarg.or((!stem.is_empty()).then_some(stem));
            let Some(action) = action else { return Ok(None) };
            (controller_kwarg.unwrap_or_default(), action)
        }
        None => {
            // No `to:` and no hashrocket target — a resource-scoped
            // shortcut (`get "suggest"` / `post "suggest", :action =>
            // "submit_suggestions"` inside `resources :stories do`).
            // Controller comes from the enclosing resources block; the
            // action is the `:action` kwarg, else the path stem. The
            // flattener nests the path under `/:<parent>_id` and names
            // the helper `<singular>_<stem>` (`story_suggest_path`).
            // Outside a resources block there's nothing to infer from
            // (a rare typo shape) — keep the silent drop.
            let Some(parent) = parent else {
                return Ok(None);
            };
            let Some(p) = path.as_deref() else {
                return Ok(None);
            };
            let stem = p.trim_matches('/').to_string();
            if stem.is_empty() || stem.contains('/') || stem.contains(':') {
                return Ok(None);
            }
            path = Some(format!("/{stem}"));
            (parent.to_string(), action_kwarg.unwrap_or(stem))
        }
    };

    Ok(Some(RouteSpec::Explicit {
        method,
        path: path.unwrap_or_default(),
        controller: ClassId(Symbol::from(controller_class_name(&controller))),
        action: Symbol::from(action),
        as_name,
        constraints,
        // The inline `on:` kwarg wins; otherwise Nested is the default
        // and a `member do`/`collection do` wrapper (handled in
        // `ingest_route_body`) overwrites it on the returned entry.
        scope: on_scope.unwrap_or(ResourceScope::Nested),
    }))
}

fn ingest_root_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<RouteSpec>> {
    // Two forms:
    //   1. `root "c#a"` — single positional string arg.
    //   2. `root to: "c#a", as: "root"` — kwargs hash (modern or
    //      hashrocket `:to =>` style; both produce KeywordHashNode).
    // Any non-string `to:` (`root to: redirect("/scan")`, a lambda) is
    // the same drop as an explicit verb's `to: redirect(...)`: the
    // route is not modeled, so it is skipped with a ledger line. It
    // used to come back as a Root with an EMPTY target, which the
    // flattener turned into `Route.new("GET", "/", :, :index)` — the
    // one file in the tree that failed `ruby -c`, and the entry point
    // (#82).
    let mut target: Option<String> = None;
    let mut redirect_target: Option<(String, u16, bool)> = None;
    let mut as_name: Option<Symbol> = None;
    if let Some(args_node) = call.arguments() {
        for arg in args_node.arguments().iter() {
            if let Some(s) = rstring(&arg) {
                if target.is_none() {
                    target = Some(s);
                }
            } else if let Some(kh) = arg.as_keyword_hash_node() {
                for el in kh.elements().iter() {
                    let Some(assoc) = el.as_assoc_node() else { continue };
                    let Some(key_sym) = symbol_value(&assoc.key()) else { continue };
                    match key_sym.as_str() {
                        "to" => {
                            if let Some(v) = rstring(&assoc.value()) {
                                target = Some(v);
                            } else if let Some(r) = redirect_literal(&assoc.value()) {
                                redirect_target = Some(r);
                            }
                        }
                        "as" => {
                            as_name = symbol_or_string_value(&assoc.value())
                                .map(|s| Symbol::from(s.as_str()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(redirect) = redirect_target {
        // `root to: redirect("/scan")` — served by a synthesized action
        // rather than dropped, so the emitted app answers `/` the way
        // Rails does (#82 recorded the drop; this lowers it).
        let (location, status, keep_query) = redirect;
        let action = if keep_query {
            redirect_sink::push_keeping_query("/", location, status)
        } else {
            redirect_sink::push("/", location, status)
        };
        return Ok(Some(RouteSpec::Explicit {
            method: HttpMethod::Get,
            path: "/".to_string(),
            controller: ClassId(Symbol::from(REDIRECT_CONTROLLER)),
            action,
            as_name: Some(as_name.unwrap_or_else(|| Symbol::from("root"))),
            constraints: IndexMap::new(),
            scope: ResourceScope::default(),
        }));
    }
    match target {
        Some(target) if !target.is_empty() => Ok(Some(RouteSpec::Root {
            target,
            as_name,
        })),
        // Same contract as `mount` and the explicit verbs' redirect
        // drop: not an error, but never silent.
        _ => {
            super::survey::record(&IngestError::Unsupported {
                file: file.into(),
                message: "route dropped: `root` with a non-string target \
                          (`to: redirect(...)` is not modeled)"
                    .into(),
            });
            Ok(None)
        }
    }
}

fn ingest_resources_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    cx: &Ctx<'_>,
    singular: bool,
) -> IngestResult<RouteSpec> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "resources call without a name".into(),
        });
    };
    let all_args = args_node.arguments();
    let mut iter = all_args.iter();
    let first = iter.next().ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "resources call without a name".into(),
    })?;
    // `resources "tours"` is `resources :tours` — Rails `to_sym`s the
    // name (#85).
    let name_str = rname(&first).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "resources name must be a symbol or string".into(),
    })?;
    let name = Symbol::from(name_str.as_str());

    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    let mut as_name: Option<Symbol> = None;
    let mut controller: Option<String> = None;
    let mut param: Option<Symbol> = None;
    let mut path: Option<String> = None;
    let mut concern_names: Vec<String> = Vec::new();
    let mut only_none = false;
    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                // An `only:`/`except:` that is written but does not
                // parse to a literal list (a constant, a method call)
                // must NOT come back empty: the expander reads an
                // empty `only` as "all seven actions", which is the
                // opposite of what a restriction means (#85).
                "only" | "except" => {
                    let list = symbol_list_value(&value);
                    let empty_literal =
                        value.as_array_node().is_some_and(|a| a.elements().iter().next().is_none());
                    // `resources :users, only: [] do … end` is how an app
                    // nests routes under a parent with no routes of its
                    // own. The expander reads an empty `only` as "all
                    // seven", so an empty literal becomes an `except:` of
                    // every action. `except: []` restricts nothing. Ruby
                    // keeps the last of duplicate keys, so each `only:` or
                    // `except:` replaces the earlier value of the same key.
                    if empty_literal {
                        if key.as_str() == "only" {
                            only_none = true;
                        } else {
                            except.clear();
                        }
                        continue;
                    }
                    if list.is_empty() {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "resources :{name_str} `{key}:` is not a literal list of actions"
                            ),
                        });
                    }
                    if key.as_str() == "only" {
                        only = list;
                        only_none = false;
                    } else {
                        except = list;
                    }
                }
                // `as:` renames the HELPERS, not the path — lobsters'
                // `namespace :mod { resources :mails, as: "mod_mails" }`
                // is `/mod/mails` served by `mod_mod_mails_path`. Dropping
                // it named those helpers `mod_mails_path`, which both
                // missed every call site and collided with the top-level
                // `resources :mod_mails`.
                "as" => as_name = rname(&value).map(|s| Symbol::from(s.as_str())),
                // `controller:` moves the CLASS and nothing else — the
                // path still comes from the resource name and so do the
                // helpers. campfire's bot API is `resources :messages,
                // controller: "messages/by_bots"`, and dropping this
                // pointed five routes at `MessagesController`, which
                // answers them with the human HTML flow.
                "controller" => controller = rname(&value),
                // `param: :task_id` renames the MEMBER SEGMENT: the
                // path binds `:task_id` and the controller reads
                // `params[:task_id]`. Dropped, the path bound `:id`
                // and the lowered action read nil (#84).
                "param" => param = symbol_or_string_value(&value).map(|s| Symbol::from(s.as_str())),
                // `path: "components"` renames the URL SEGMENT and nothing
                // else (`/components`, still `parts_path` and
                // `PartsController`). Rails strips the slashes, so
                // `path: "/components"` is the same segment.
                "path" => {
                    let Some(raw) = symbol_or_string_value(&value) else {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "resources :{name_str} `path:` is not a literal string or symbol"
                            ),
                        });
                    };
                    // A dynamic segment (`"categories/:category_id/parts"`),
                    // a glob or an optional group adds route params the
                    // flattener does not carry yet; refuse rather than
                    // serve a path whose `:category_id` never reaches
                    // `params`.
                    if raw.contains([':', '*', '(']) {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "resources :{name_str} `path: {raw:?}` has a dynamic segment"
                            ),
                        });
                    }
                    // `path: ""` / `path: "/"` mounts the resource at the
                    // root (`GET /` is `index`, `/:id` is `show`). That
                    // is not the resource name, so refuse it rather
                    // than fall back to `/parts`.
                    let segment = raw.trim_matches('/');
                    if segment.is_empty() {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "resources :{name_str} `path: {raw:?}` mounts the resource at the root"
                            ),
                        });
                    }
                    path = Some(segment.to_string());
                }
                // `concerns: :c` / `concerns: [:a, :b]` replays those
                // concerns inside the resource's scope, after its block.
                "concerns" => {
                    concern_names = match value.as_array_node() {
                        Some(a) => a.elements().iter().filter_map(|e| rname(&e)).collect(),
                        None => rname(&value).into_iter().collect(),
                    };
                }
                // `shallow:` lands when a fixture demands it.
                _ => {}
            }
        }
    }

    if only_none {
        only.clear();
        except = ALL_RESOURCE_ACTIONS.iter().copied()
            .into_iter()
            .map(Symbol::from)
            .collect();
    }

    let mut nested = block_entries(call, file, Some(name_str.as_str()), cx)?;
    if !concern_names.is_empty() {
        let none = eval::RVal::Hash(Vec::new());
        nested.extend(cx.with_mount_scope(cx.mount_scope_depth.get() + 1, || {
            run_concerns(&concern_names, &none, file, Some(name_str.as_str()), cx)
        })?);
    }

    Ok(RouteSpec::Resources {
        name,
        only,
        except,
        nested,
        singular,
        as_name,
        controller,
        param,
        path,
    })
}

/// `"c"` / `"admin/c"` → `CController` / `Admin::CController`.
pub(super) fn controller_class_name(short: &str) -> String {
    let mut s = short
        .split('/')
        .map(camelize)
        .collect::<Vec<_>>()
        .join("::");
    s.push_str("Controller");
    s
}

/// Every action `resources`/`resource` can generate a route for.
const ALL_RESOURCE_ACTIONS: [&str; 7] =
    ["index", "show", "new", "create", "edit", "update", "destroy"];
