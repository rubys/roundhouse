//! `config/routes.rb` emission. Round-trips the surface forms ingest
//! preserved (`get/post/...`, `root`, `resources`) — emit consumers
//! that need a flat `(verb, path, controller, action)` table use
//! `crate::lower::flatten_routes` instead.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use crate::dialect::{HttpMethod, RouteSpec, RouteTable};
use crate::ident::Symbol;
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
