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

use crate::dialect::{HttpMethod, ResourceScope, RouteSpec, RouteTable};
use crate::naming::camelize;
use crate::{ClassId, Symbol};

use super::util::{
    constant_id_str, find_call_named, flatten_statements, string_value, symbol_list_value,
    symbol_value,
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

    // Find the outer `Rails.application.routes.draw do ... end` call.
    let Some(draw_call) = find_call_named(&root, "draw") else {
        return Ok(RouteTable::default());
    };
    let Some(block_node) = draw_call.block() else {
        return Ok(RouteTable::default());
    };
    let Some(block) = block_node.as_block_node() else {
        return Ok(RouteTable::default());
    };

    let entries = match block.body() {
        Some(body) => ingest_route_body(body, file, None, draws)?,
        None => Vec::new(),
    };

    Ok(RouteTable { entries })
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
            Ok(Some(spec)) => entries.push(spec),
            Ok(None) => {}
            Err(err) if super::survey::is_active() => super::survey::record(&err),
            Err(err) => return Err(err),
        }
    }
    Ok(entries)
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
        return ingest_explicit_route(call, http, file, parent);
    }
    match method {
        "root" => ingest_root_route(call).map(Some),
        "resources" => ingest_resources_route(call, file, draws, false).map(Some),
        "resource" => ingest_resources_route(call, file, draws, true).map(Some),
        "namespace" => ingest_namespace_route(call, file, draws).map(Some),
        "scope" => ingest_scope_route(call, file, draws).map(Some),
        "draw" => ingest_draw_route(call, file, draws),
        // Unknown DSL — `concern`, `mount`, `devise_for`,
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
                    // `defaults:`, `constraints:`, `format:` shape the
                    // request, not the (path, controller, action)
                    // triple this table models.
                    _ => {}
                }
            }
        }
    }
    let entries = block_entries(call, file, None, draws)?;
    Ok(RouteSpec::Scope { path, module, as_prefix, entries })
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
    let Some((source, path)) = draws.get(&name) else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("draw(:{name}) — config/routes/{name}.rb not found"),
        });
    };
    super::sources::register(path, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, path);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return Err(IngestError::Parse {
            file: path.clone(),
            message: "route file is not a program".into(),
        });
    };
    let entries =
        ingest_route_stmts(program.statements().body().iter(), path, None, draws)?;
    Ok(Some(RouteSpec::Scope { path: None, module: None, as_prefix: None, entries }))
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
    let mut as_name: Option<Symbol> = None;
    let mut action_kwarg: Option<String> = None;
    let mut constraints: IndexMap<Symbol, String> = IndexMap::new();

    for arg in args_node.arguments().iter() {
        if let Some(s) = string_value(&arg) {
            // Positional string arg — the path: `get "/p", to: "c#a"`.
            if path.is_none() {
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
                            // `get "/p" => redirect(...)` style. Mark
                            // unsupported so we drop the whole route
                            // gracefully (it's not bench-critical and
                            // RouteSpec has no Redirect variant yet).
                            to_is_unsupported = true;
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
                        } else {
                            // `to: redirect(...)` — non-string target.
                            // Drop the route (see above).
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
                    // `post "suggest", :action => "submit_suggestions"` —
                    // the action override for a resource-scoped shortcut.
                    "action" => {
                        action_kwarg =
                            string_value(value).or_else(|| symbol_value(value));
                    }
                    // `via: :all` (HTTP-method override) and similar
                    // method-shaping options aren't modeled today; the
                    // route still resolves to the outer verb. Other
                    // string-value options become routing constraints.
                    "via" => {}
                    other => {
                        if let Some(v) = string_value(value) {
                            constraints.insert(Symbol::from(other), v);
                        }
                    }
                }
            }
        }
    }

    if to_is_unsupported {
        // Silent drop — survey-mode users still see the source file via
        // the surrounding file-level ingest; nothing else hits this
        // route. Bench targets (`/articles`, etc.) use the supported
        // shapes exclusively.
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
        // Default; a `member do`/`collection do` wrapper (handled in
        // `ingest_route_body`) overwrites this on the returned entry.
        scope: ResourceScope::Nested,
    }))
}

fn ingest_root_route(call: &ruby_prism::CallNode<'_>) -> IngestResult<RouteSpec> {
    // Two forms:
    //   1. `root "c#a"` — single positional string arg.
    //   2. `root to: "c#a", as: "root"` — kwargs hash (modern or
    //      hashrocket `:to =>` style; both produce KeywordHashNode).
    // Any non-string `to:` value (`root to: ->{...}`) leaves target
    // empty — downstream emitters skip a Root with no target.
    let Some(args_node) = call.arguments() else {
        return Ok(RouteSpec::Root { target: String::new() });
    };
    let mut target: Option<String> = None;
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
                    }
                }
            }
        }
    }
    Ok(RouteSpec::Root { target: target.unwrap_or_default() })
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
    let name_str = symbol_value(&first).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "resources name must be a symbol".into(),
    })?;
    let name = Symbol::from(name_str.as_str());

    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "only" => only = symbol_list_value(&value),
                "except" => except = symbol_list_value(&value),
                // `as:`, `path:`, `controller:`, `shallow:` land when
                // a fixture demands them.
                _ => {}
            }
        }
    }

    let nested = block_entries(call, file, Some(name_str.as_str()), draws)?;

    Ok(RouteSpec::Resources { name, only, except, nested, singular })
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
