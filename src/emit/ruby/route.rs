//! `config/routes.rb` emission. Round-trips the surface forms ingest
//! preserved (`get/post/...`, `root`, `resources`) — emit consumers
//! that need a flat `(verb, path, controller, action)` table use
//! `crate::lower::flatten_routes` instead.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use crate::App;
use crate::dialect::{HttpMethod, RouteSpec, RouteTable};
use crate::ident::Symbol;
use crate::lower::{flatten_routes, FlatRoute};
use crate::naming::snake_case;

pub(super) fn emit_routes(routes: &RouteTable) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "Rails.application.routes.draw do").unwrap();
    for (i, entry) in routes.entries.iter().enumerate() {
        if i > 0 && needs_blank_separator(&routes.entries[i - 1], entry) {
            writeln!(s).unwrap();
        }
        write_route_spec(&mut s, entry, 1);
    }
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("config/routes.rb"), content: s }
}

/// Blank line between `root "..."` and a following `resources` block —
/// matches the Rails scaffold's idiomatic spacing and the fixture source.
fn needs_blank_separator(prev: &RouteSpec, next: &RouteSpec) -> bool {
    matches!(prev, RouteSpec::Root { .. })
        && matches!(next, RouteSpec::Resources { .. })
}

fn write_route_spec(out: &mut String, spec: &RouteSpec, depth: usize) {
    let indent = "  ".repeat(depth);
    match spec {
        RouteSpec::Explicit {
            method,
            path,
            controller,
            action,
            as_name,
            constraints: _,
        } => {
            let verb = verb_keyword(method);
            let mut opts = vec![format!(
                "to: {:?}",
                format!("{}#{}", strip_controller_suffix(controller.0.as_str()), action)
            )];
            if let Some(name) = as_name {
                opts.push(format!("as: :{name}"));
            }
            if matches!(method, HttpMethod::Any) {
                opts.push("via: :all".into());
            }
            writeln!(out, "{indent}{verb} {:?}, {}", path, opts.join(", ")).unwrap();
        }
        RouteSpec::Root { target } => {
            writeln!(out, "{indent}root {:?}", target).unwrap();
        }
        RouteSpec::Resources { name, only, except, nested } => {
            let mut header = format!("{indent}resources :{name}");
            if !only.is_empty() {
                header.push_str(&format!(", only: [{}]", join_symbols(only)));
            }
            if !except.is_empty() {
                header.push_str(&format!(", except: [{}]", join_symbols(except)));
            }
            if nested.is_empty() {
                writeln!(out, "{header}").unwrap();
            } else {
                writeln!(out, "{header} do").unwrap();
                for child in nested {
                    write_route_spec(out, child, depth + 1);
                }
                writeln!(out, "{indent}end").unwrap();
            }
        }
    }
}

fn verb_keyword(m: &HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
        HttpMethod::Any => "match",
    }
}

fn join_symbols(syms: &[Symbol]) -> String {
    syms.iter().map(|s| format!(":{s}")).collect::<Vec<_>>().join(", ")
}

fn strip_controller_suffix(s: &str) -> String {
    let base = s.strip_suffix("Controller").unwrap_or(s);
    snake_case(base)
}

/// Emit `config/routes.rb` in spinel-blog shape: a `Routes` module
/// containing a frozen `TABLE` array of `{method:, pattern:, controller:,
/// action:}` hashes (one per dispatched verb+path pair) plus a `ROOT`
/// constant carrying the root route. `controller` is a snake-case
/// symbol (`:articles`), not a class reference — spinel's hash
/// specializations only handle scalars, so the router returns the symbol
/// and main.rb's `instantiate_controller` case-dispatches to the literal
/// `.new` call.
///
/// Built on top of `flatten_routes`, which already expands `resources`
/// blocks into the standard-action set and threads nested scopes
/// (`/articles/:article_id/comments/...`) — the spinel render is
/// purely a formatter.
pub(super) fn emit_lowered_routes(app: &App) -> EmittedFile {
    let flat = flatten_routes(app);
    let (root, table): (Vec<&FlatRoute>, Vec<&FlatRoute>) =
        flat.iter().partition(|r| r.path == "/");

    let mut s = String::new();

    // require_relative headers: application_controller plus each
    // unique controller referenced by the route table, in the order
    // they first appear. application_controller is always present
    // even when no controller inherits explicitly from it — main.rb
    // requires it as the dispatch base.
    writeln!(s, "require_relative \"../app/controllers/application_controller\"").unwrap();
    let mut seen: Vec<String> = Vec::new();
    for r in &flat {
        let file = controller_file_stem(r.controller.0.as_str());
        if file == "application_controller" || seen.contains(&file) {
            continue;
        }
        seen.push(file.clone());
        writeln!(s, "require_relative \"../app/controllers/{file}\"").unwrap();
    }
    writeln!(s).unwrap();

    writeln!(s, "module Routes").unwrap();
    writeln!(s, "  TABLE = [").unwrap();
    for r in &table {
        writeln!(s, "    {},", route_hash_literal(r)).unwrap();
    }
    writeln!(s, "  ].freeze").unwrap();

    if let Some(r) = root.first() {
        writeln!(s).unwrap();
        writeln!(s, "  ROOT = {}.freeze", route_hash_literal(r)).unwrap();
    }
    writeln!(s, "end").unwrap();

    EmittedFile { path: PathBuf::from("config/routes.rb"), content: s }
}

fn route_hash_literal(r: &FlatRoute) -> String {
    format!(
        "{{ method: {:?}, pattern: {:?}, controller: :{}, action: :{} }}",
        verb_string(&r.method),
        r.path,
        controller_symbol(r.controller.0.as_str()),
        r.action.as_str(),
    )
}

fn verb_string(m: &HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Any => "ANY",
    }
}

/// `ArticlesController` → `articles` (the controller-symbol form
/// spinel's router uses).
fn controller_symbol(class_name: &str) -> String {
    let base = class_name.strip_suffix("Controller").unwrap_or(class_name);
    snake_case(base)
}

/// `ArticlesController` → `articles_controller` (the require_relative
/// file-stem form).
fn controller_file_stem(class_name: &str) -> String {
    format!("{}_controller", controller_symbol(class_name))
}
