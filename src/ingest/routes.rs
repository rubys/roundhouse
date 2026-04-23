//! `config/routes.rb` — parse the `Rails.application.routes.draw do … end`
//! DSL into a `RouteTable`. Recognizes verb shortcuts (`get`/`post`/…),
//! `root`, and `resources` (with nested blocks and `only:` / `except:`).

use indexmap::IndexMap;
use ruby_prism::{Node, parse};

use crate::dialect::{HttpMethod, RouteSpec, RouteTable};
use crate::naming::camelize;
use crate::{ClassId, Symbol};

use super::util::{
    constant_id_str, find_call_named, flatten_statements, string_value, symbol_list_value,
    symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_routes(source: &[u8], file: &str) -> IngestResult<RouteTable> {
    let result = parse(source);
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
        Some(body) => ingest_route_body(body, file)?,
        None => Vec::new(),
    };

    Ok(RouteTable { entries })
}

/// Walk the statements inside a `routes.draw do ... end` block (or a
/// nested `resources :x do ... end` block) and collect their `RouteSpec`
/// entries. Recognized forms: verb shortcuts, `root "c#a"`, and
/// `resources :name`.
fn ingest_route_body(body: Node<'_>, file: &str) -> IngestResult<Vec<RouteSpec>> {
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
        if let Some(spec) = ingest_route_call(&call, &method, file)? {
            entries.push(spec);
        }
    }
    Ok(entries)
}

fn ingest_route_call(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
    file: &str,
) -> IngestResult<Option<RouteSpec>> {
    // Verb shortcuts (`get "/p", to: "c#a"`).
    if let Some(http) = http_method_from(method) {
        return ingest_explicit_route(call, http, file).map(Some);
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
) -> IngestResult<RouteSpec> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "verb route without arguments".into(),
        });
    };
    let mut path: Option<String> = None;
    let mut to: Option<String> = None;
    let mut as_name: Option<Symbol> = None;
    let mut constraints: IndexMap<Symbol, String> = IndexMap::new();

    for arg in args_node.arguments().iter() {
        if let Some(s) = string_value(&arg) {
            if path.is_none() {
                path = Some(s);
            }
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key_sym) = symbol_value(&assoc.key()) else { continue };
                let value = &assoc.value();
                match key_sym.as_str() {
                    "to" => to = string_value(value),
                    "as" => as_name = symbol_value(value).map(Symbol::from),
                    other => {
                        if let Some(v) = string_value(value) {
                            constraints.insert(Symbol::from(other), v);
                        }
                    }
                }
            }
        }
    }

    let (controller, action) = match to.as_deref().and_then(|s| s.split_once('#')) {
        Some((c, a)) => (c.to_string(), a.to_string()),
        None => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "route missing `to: \"controller#action\"`".into(),
            });
        }
    };

    Ok(RouteSpec::Explicit {
        method,
        path: path.unwrap_or_default(),
        controller: ClassId(Symbol::from(controller_class_name(&controller))),
        action: Symbol::from(action),
        as_name,
        constraints,
    })
}

fn ingest_root_route(call: &ruby_prism::CallNode<'_>) -> IngestResult<RouteSpec> {
    // `root "c#a"` — exactly one string arg. Keyword forms
    // (`root to: "c#a"`) aren't in any fixture yet; add when needed.
    let target = call
        .arguments()
        .and_then(|a| a.arguments().iter().next().and_then(|n| string_value(&n)))
        .unwrap_or_default();
    Ok(RouteSpec::Root { target })
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
                Some(body) => ingest_route_body(body, file)?,
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
