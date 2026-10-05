//! `config/routes.rb` — parse the `Rails.application.routes.draw do … end`
//! DSL into a `RouteTable`. Recognizes verb shortcuts (`get`/`post`/…),
//! `root`, `resources`/`resource`, `namespace`/`scope`, and
//! `draw(:name)` inclusion of `config/routes/<name>.rb` split files.
//!
//! Recovery discipline: in survey mode an unsupported DSL construct
//! (`mount`, `use_doorkeeper`, `devise_for`, …) records a gap and drops
//! that one entry — the rest of the table still flattens. In strict
//! mode it still fails loud so the fixture that introduces a new form
//! forces a recognizer. Not-modeled ≠ absent: a dropped entry is a
//! ledger line, never a silently empty route table.

use std::collections::HashMap;

use indexmap::IndexMap;
use ruby_prism::Node;

use crate::dialect::{DirectHelper, HttpMethod, ResourceScope, RouteSpec, RouteTable};
use crate::naming::camelize;
use crate::{ClassId, Symbol};

use super::util::{
    constant_id_str, constant_path_of, find_call_named, flatten_statements, string_value,
    symbol_list_value, symbol_or_string_value, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_routes(source: &[u8], file: &str) -> IngestResult<RouteTable> {
    ingest_routes_with_draws(source, file, &HashMap::new())
}

/// `draws` maps a `draw(:name)` symbol to the split file Rails loads
/// into the same DSL context (`config/routes/<name>.rb`): name →
/// (source, path). The app ingester reads the directory; tests pass
/// maps directly.
pub fn ingest_routes_with_draws(
    source: &[u8],
    file: &str,
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<RouteTable> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();

    // Every top-level `X.routes.draw do ... end` call — usually exactly
    // one (`Rails.application.routes.draw` or `Procore::Application
    // .routes.draw`), but an app can carry a second top-level block
    // (legacy + Rails-native routing split across two calls in the same
    // file). `find_call_named` on the whole root would find only the
    // first (it returns on the first match), so each top-level
    // statement is searched independently and every hit kept — a
    // second block silently dropped would violate the "loud by design"
    // discipline this module documents at the top.
    let draw_calls = top_level_draw_calls(&root);
    if draw_calls.is_empty() {
        return Ok(RouteTable::default());
    }

    let mut entries = Vec::new();
    // Collected by a second walk rather than threaded through the
    // recursive entry ingest: a `direct` is not a RouteSpec (it adds no
    // path), so it has nowhere to ride in the `entries` return, and
    // every recursive arm would otherwise need a mutable accumulator
    // for a construct that appears a handful of times per app.
    let mut direct_helpers = Vec::new();
    for draw_call in &draw_calls {
        let Some(block_node) = draw_call.block() else { continue };
        let Some(block) = block_node.as_block_node() else { continue };
        let Some(body) = block.body() else { continue };
        direct_helpers.extend(collect_direct_helpers(&body, file)?);
        entries.extend(ingest_route_body(body, file, None, draws)?);
    }

    Ok(RouteTable { entries, direct_helpers, redirects: redirect_sink::drain() })
}

/// Every top-level `draw` call in `root` — one per `X.routes.draw do
/// ... end` block. Each top-level statement is itself the outer call
/// (its own name is `draw`, so `find_call_named` matches immediately
/// without descending into siblings), so searching statement-by-
/// statement rather than the whole tree at once is what makes a SECOND
/// top-level block visible instead of only the first.
fn top_level_draw_calls<'pr>(root: &Node<'pr>) -> Vec<ruby_prism::CallNode<'pr>> {
    let stmts: Vec<Node<'pr>> = if let Some(p) = root.as_program_node() {
        p.statements().body().iter().collect()
    } else if let Some(s) = root.as_statements_node() {
        s.body().iter().collect()
    } else {
        return find_call_named(root, "draw").into_iter().collect();
    };
    stmts.iter().filter_map(|stmt| find_call_named(stmt, "draw")).collect()
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
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<Vec<RouteSpec>> {
    ingest_route_stmts(flatten_statements(body).into_iter(), file, parent, draws)
}

fn ingest_route_stmts<'pr>(
    stmts: impl Iterator<Item = Node<'pr>>,
    file: &str,
    parent: Option<&str>,
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<Vec<RouteSpec>> {
    let mut entries = Vec::new();
    for stmt in stmts {
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
                    match ingest_glob_draw_each(&call, &pattern, file, draws) {
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
        if matches!(method.as_str(), "constraints" | "member" | "collection") {
            if let Some(block_node) = call.block() {
                if let Some(block) = block_node.as_block_node() {
                    if let Some(inner_body) = block.body() {
                        let mut inner =
                            ingest_route_body(inner_body, file, parent, draws)?;
                        let scope = match method.as_str() {
                            "member" => Some(ResourceScope::Member),
                            "collection" => Some(ResourceScope::Collection),
                            _ => None, // constraints: no scope change
                        };
                        if let Some(scope) = scope {
                            for entry in &mut inner {
                                if let RouteSpec::Explicit { scope: s, .. } = entry {
                                    *s = scope;
                                }
                            }
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
        match ingest_route_call(&call, &method, file, parent, draws) {
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
    draws: &HashMap<String, (Vec<u8>, String)>,
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
        "resources" => ingest_resources_route(call, file, draws, false).map(Some),
        "resource" => ingest_resources_route(call, file, draws, true).map(Some),
        "namespace" => ingest_namespace_route(call, file, draws).map(Some),
        "scope" => ingest_scope_route(call, file, draws).map(Some),
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
            entries: block_entries(call, file, None, draws)?,
        })),
        "draw" => ingest_draw_route(call, file, draws),
        // `mount SomeEngine, at: "/path"` — the mounted engine is
        // external code (mission_control, sidekiq-web, …), never part
        // of the transpiled app. Dropping the route is the modeled
        // truth (same contract as `to: redirect(...)` above); survey
        // runs still get a ledger line so the drop is visible.
        "mount" => {
            if super::survey::is_active() {
                super::survey::record(&IngestError::Unsupported {
                    file: file.into(),
                    message: "route dropped: `mount` of an external engine".into(),
                });
            }
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
        // Unknown DSL — `concern`, `devise_for`,
        // `use_doorkeeper`, `authenticate`, etc. land here. Strict
        // ingest fails loud so the fixture that introduces them forces
        // a recognizer; survey callers get a per-entry ledger line
        // (see ingest_route_stmts).
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
        if let Some(s) = symbol_value(&arg) {
            return Some(s);
        }
        if let Some(s) = string_value(&arg) {
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
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<Vec<RouteSpec>> {
    match call.block() {
        Some(block_node) => match block_node.as_block_node() {
            Some(block) => match block.body() {
                Some(body) => ingest_route_body(body, file, parent, draws),
                None => Ok(Vec::new()),
            },
            None => Ok(Vec::new()),
        },
        None => Ok(Vec::new()),
    }
}

/// `namespace :admin do … end` — `scope` with path, controller module,
/// and helper prefix all set to the name. Resets the enclosing
/// `resources` inference context (Rails does not infer member
/// controllers across a namespace boundary).
fn ingest_namespace_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<RouteSpec> {
    let Some(name) = first_name_arg(call) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "namespace without a name".into(),
        });
    };
    let entries = block_entries(call, file, None, draws)?;
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
    draws: &HashMap<String, (Vec<u8>, String)>,
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
                let val = symbol_value(&value).or_else(|| string_value(&value));
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
    let entries = block_entries(call, file, None, draws)?;
    Ok(RouteSpec::Scope { path, module, as_prefix, defaults, nest: false, entries })
}

/// `draw(:admin)` — Rails loads `config/routes/admin.rb` into the same
/// DSL context. The split file's top-level statements are route DSL
/// directly (no `routes.draw` wrapper). Included entries ride a
/// facet-less Scope so the flattener composes them transparently.
fn ingest_draw_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    draws: &HashMap<String, (Vec<u8>, String)>,
) -> IngestResult<Option<RouteSpec>> {
    let Some(name) = first_name_arg(call) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "draw without a route-file name".into(),
        });
    };
    let Some((source, path)) = resolve_draw_name(&name, draws) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("draw(:{name}) — config/routes/{name}.rb not found"),
        });
    };
    let entries = ingest_routes_file_entries(source, path, draws)?;
    Ok(Some(RouteSpec::Scope {
        path: None,
        module: None,
        as_prefix: None,
        defaults: IndexMap::new(),
        nest: false,
        entries,
    }))
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
    draws: &HashMap<String, (Vec<u8>, String)>,
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
    ingest_route_stmts(program.statements().body().iter(), path, None, draws)
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
    draws: &HashMap<String, (Vec<u8>, String)>,
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
        draws.keys().filter(|key| glob_matches(pattern, &format!("{key}.rb"))).collect();
    matched.sort();

    let mut entries = Vec::with_capacity(matched.len());
    for key in matched {
        let (source, path) = &draws[key];
        entries.push(RouteSpec::Scope {
            path: None,
            module: None,
            as_prefix: None,
            defaults: IndexMap::new(),
            nest: false,
            entries: ingest_routes_file_entries(source, path, draws)?,
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
    // The INLINE spelling of `member do … end` / `collection do … end`.
    // Rails accepts both, and campfire writes `delete :clear, on:
    // :collection`; only the block form was recognized, so the route was
    // nested as `/searches/:search_id/clear` (named `search_clear`)
    // where Rails serves `/searches/clear` (named `clear_searches`).
    let mut on_scope: Option<ResourceScope> = None;
    let mut constraints: IndexMap<Symbol, String> = IndexMap::new();

    for arg in args_node.arguments().iter() {
        if let Some(s) = string_value(&arg) {
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
                        if let Some(v) = string_value(value) {
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
                        if let Some(v) = string_value(value) {
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
                        as_name = symbol_value(value)
                            .map(Symbol::from)
                            .or_else(|| string_value(value).map(Symbol::from));
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
                    "action" => {
                        action_kwarg =
                            string_value(value).or_else(|| symbol_value(value));
                    }
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
    if let Some(args_node) = call.arguments() {
        for arg in args_node.arguments().iter() {
            if let Some(s) = string_value(&arg) {
                if target.is_none() {
                    target = Some(s);
                }
            } else if let Some(kh) = arg.as_keyword_hash_node() {
                for el in kh.elements().iter() {
                    let Some(assoc) = el.as_assoc_node() else { continue };
                    let Some(key_sym) = symbol_value(&assoc.key()) else { continue };
                    if key_sym.as_str() == "to" {
                        if let Some(v) = string_value(&assoc.value()) {
                            target = Some(v);
                        } else if let Some(r) = redirect_literal(&assoc.value()) {
                            redirect_target = Some(r);
                        }
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
            as_name: Some(Symbol::from("root")),
            constraints: IndexMap::new(),
            scope: ResourceScope::default(),
        }));
    }
    match target {
        Some(target) if !target.is_empty() => Ok(Some(RouteSpec::Root { target })),
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
    draws: &HashMap<String, (Vec<u8>, String)>,
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
    let name_str = symbol_or_string_value(&first).ok_or_else(|| IngestError::Unsupported {
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
                "as" => as_name = symbol_or_string_value(&value).map(|s| Symbol::from(s.as_str())),
                // `controller:` moves the CLASS and nothing else — the
                // path still comes from the resource name and so do the
                // helpers. campfire's bot API is `resources :messages,
                // controller: "messages/by_bots"`, and dropping this
                // pointed five routes at `MessagesController`, which
                // answers them with the human HTML flow.
                "controller" => controller = symbol_or_string_value(&value),
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
                // `shallow:` lands when a fixture demands it.
                _ => {}
            }
        }
    }

    if only_none {
        only.clear();
        except = ["index", "new", "create", "show", "edit", "update", "destroy"]
            .into_iter()
            .map(Symbol::from)
            .collect();
    }

    let nested = block_entries(call, file, Some(name_str.as_str()), draws)?;

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
fn controller_class_name(short: &str) -> String {
    let mut s = short
        .split('/')
        .map(camelize)
        .collect::<Vec<_>>()
        .join("::");
    s.push_str("Controller");
    s
}
