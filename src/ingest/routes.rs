//! `config/routes.rb` — parse the `Rails.application.routes.draw do … end`
//! DSL into a `RouteTable`. Recognizes verb shortcuts (`get`/`post`/…),
//! `root`, and `resources` (with nested blocks and `only:` / `except:`).

use indexmap::IndexMap;
use ruby_prism::Node;

use crate::dialect::{HttpMethod, RouteSpec, RouteTable};
use crate::naming::camelize;
use crate::{ClassId, Symbol};

use super::util::{
    constant_id_str, find_call_named, flatten_statements, string_value, symbol_list_value,
    symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_routes(source: &[u8], file: &str) -> IngestResult<RouteTable> {
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
        Some(body) => ingest_route_body(body, file, None)?,
        None => Vec::new(),
    };

    Ok(RouteTable { entries })
}

/// Walk the statements inside a `routes.draw do ... end` block (or a
/// nested `resources :x do ... end` block) and collect their `RouteSpec`
/// entries. Recognized forms: verb shortcuts, `root "c#a"`, and
/// `resources :name`.
/// `parent` carries the enclosing `resources :<name>` (its plural name)
/// so bare-verb member/nested shortcuts (`get "suggest"` with no `to:`)
/// can infer their controller; `None` at the top level.
fn ingest_route_body(
    body: Node<'_>,
    file: &str,
    parent: Option<&str>,
) -> IngestResult<Vec<RouteSpec>> {
    let mut entries = Vec::new();
    for stmt in flatten_statements(body) {
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
        //     scoping wrappers) — they affect the path prefix
        //     (`/resource/:id/...` vs `/resource/...`), but the
        //     routes inside lobsters are all bare-string action
        //     shortcuts that we drop anyway (see
        //     ingest_explicit_route's bare-action handling), so
        //     flattening loses no signal here.
        //
        // `scope` and `namespace` are NOT in this set — they prepend
        // path segments to the nested routes, so flattening would
        // produce wrong paths. They'll surface as Unsupported until
        // a fixture forces a proper implementation.
        if matches!(method.as_str(), "constraints" | "member" | "collection") {
            if let Some(block_node) = call.block() {
                if let Some(block) = block_node.as_block_node() {
                    if let Some(inner_body) = block.body() {
                        entries.extend(ingest_route_body(inner_body, file, parent)?);
                    }
                }
            }
            continue;
        }

        if let Some(spec) = ingest_route_call(&call, &method, file, parent)? {
            entries.push(spec);
        }
    }
    Ok(entries)
}

fn ingest_route_call(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
    file: &str,
    parent: Option<&str>,
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
        "resources" => ingest_resources_route(call, file).map(Some),
        // Unknown DSL — `resource` (singular), `namespace`, `scope`,
        // `concern`, `mount`, etc. land here. Fail loud so the fixture
        // that introduces them forces a recognizer.
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

    let nested = match call.block() {
        Some(block_node) => match block_node.as_block_node() {
            Some(block) => match block.body() {
                Some(body) => ingest_route_body(body, file, Some(name_str.as_str()))?,
                None => Vec::new(),
            },
            None => Vec::new(),
        },
        None => Vec::new(),
    };

    Ok(RouteSpec::Resources { name, only, except, nested })
}

fn controller_class_name(short: &str) -> String {
    let mut s = camelize(short);
    s.push_str("Controller");
    s
}
