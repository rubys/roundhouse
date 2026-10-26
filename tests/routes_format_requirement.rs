//! A route's requirement on its `(.:format)` extension:
//! `constraints: { format: /csv/ }`, `format: false`, and the String
//! `format: "json"` (a requirement and the default both). Rails holds an
//! extension to the requirement, lets a request with none through, and
//! forces no format of its own unless the route names one. Before, the
//! constraint was taken for the route's forced response format, nothing
//! checked it, and `format: false` was dropped, so `/exports/1/export`
//! answered as CSV and `/exports/1/export.json` and `/raw/1.json` were
//! served.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/format_requirement.rs"]
mod format_requirement;

use std::process::Command;

use roundhouse::App;
use roundhouse::dialect::HttpMethod;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::routes::{FlatRoute, flatten_routes};

fn routes(source: &str) -> Result<Vec<FlatRoute>, String> {
    let mut app = App::default();
    app.routes = ingest_routes(
        format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(),
        "config/routes.rb",
    )
    .map_err(|e| e.to_string())?;
    Ok(flatten_routes(&app))
}

fn only(source: &str) -> FlatRoute {
    let r = routes(source).expect("ingest routes");
    assert_eq!(r.len(), 1, "{r:#?}");
    let route = r.into_iter().next().unwrap();
    assert_eq!(route.method, HttpMethod::Get);
    route
}

fn format_pattern(route: &FlatRoute) -> Option<&str> {
    route.seg_patterns.iter().find(|(n, _)| n == "format").map(|(_, p)| p.as_str())
}

#[test]
fn a_format_constraint_is_a_requirement_not_the_forced_format() {
    let r = only(r#"get "/exports/:id/export", to: "widgets#export", constraints: { format: /csv/ }"#);
    assert_eq!(r.format, None, "{r:#?}");
    assert!(format_pattern(&r).is_some(), "{r:#?}");
    assert!(r.refused.is_none(), "{r:#?}");
}

#[test]
fn format_false_refuses_every_extension() {
    let r = only(r#"get "/raw/:id", to: "widgets#raw", format: false"#);
    assert_eq!(r.format, None, "{r:#?}");
    assert_eq!(format_pattern(&r), Some(roundhouse::lower::segment_pattern::NEVER), "{r:#?}");
}

#[test]
fn a_string_format_option_is_a_requirement_and_the_default() {
    let r = only(r#"get "/pinned/:id", to: "widgets#pinned", format: "json""#);
    assert_eq!(r.format.as_ref().map(|f| f.as_str()), Some("json"), "{r:#?}");
    assert!(format_pattern(&r).is_some(), "{r:#?}");
}

#[test]
fn format_true_is_a_located_gap() {
    let err = routes(r#"get "/x/:id", to: "widgets#x", format: true"#).expect_err("format: true is not modeled");
    assert!(err.contains("format: true"), "{err}");
}

/// Guards: Rails gives `format: :json` (a Symbol) neither a requirement
/// nor a default, and `defaults: { format: :json }` a default only; both
/// lower as they did.
#[test]
fn a_symbol_format_option_stays_ignored() {
    let r = only(r#"get "/fs/:id", to: "widgets#x", format: :json"#);
    assert_eq!(r.format, None, "{r:#?}");
    assert!(r.seg_patterns.is_empty(), "{r:#?}");
}

#[test]
fn a_defaults_format_stays_the_default_only() {
    let r = only(r#"get "/fd/:id", to: "widgets#x", defaults: { format: :json }"#);
    assert_eq!(r.format.as_ref().map(|f| f.as_str()), Some("json"), "{r:#?}");
    assert!(r.seg_patterns.is_empty(), "{r:#?}");
}

fn ruby_target_answers(overlay: emit_and_run::Overlay, assertions: String) {
    let script = format!(
        r#"require "stringio"
def req(path)
  result = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status = result[0]
  status == 404 ? "404|" : status.to_s + "|" + result[2].join
end
{assertions}puts "PASS"
"#
    );
    overlay.run_ruby(&script).assert_passes();
}

/// Run `assertions` against actionpack 8.1.4's router drawing `routes`
/// over `controller`.
fn rails_answers(controller: &str, routes: &str, assertions: String) {
    if !Command::new("ruby").args(["-raction_controller", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the actionpack gem not available");
        return;
    }
    let script = format!(
        r#"gem "actionpack", "8.1.4"
require "action_controller"
require "rack/mock"
{controller}
routes = ActionDispatch::Routing::RouteSet.new
routes.draw do
{draw}
end
def req_with(routes, path)
  status, _headers, body = routes.call(Rack::MockRequest.env_for(path))
  return "404|" if status == 404
  text = +""
  body.each {{ |part| text << part }}
  status.to_s + "|" + text
end
define_method(:req) {{ |path| req_with(routes, path) }}
{assertions}puts "Rails agrees"
"#,
        controller = controller.replace("< ApplicationController", "< ActionController::Base"),
        draw = routes.lines().filter(|l| l.starts_with("  ")).collect::<Vec<_>>().join("\n"),
    );
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "actionpack 8.1.4 disagrees with the table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

#[test]
fn the_ruby_target_answers_as_rails_does() {
    ruby_target_answers(format_requirement::overlay(), format_requirement::assertions());
}

/// The table itself, against actionpack 8.1.4's router.
#[test]
fn rails_answers_the_table() {
    rails_answers(format_requirement::CONTROLLER, format_requirement::ROUTES, format_requirement::assertions());
}

// --- Constraints written as plain text --------------------------------

#[test]
fn a_plain_name_constraint_is_its_own_exact_pattern() {
    let r = only(r#"get "/exports/:id/followers", to: "exports#followers", constraints: { format: :csv }"#);
    assert_eq!(r.format, None, "{r:#?}");
    assert!(format_pattern(&r).is_some() && r.refused.is_none(), "{r:#?}");
    let r = only(r#"get "/kinds/:kind", to: "exports#kind", constraints: { kind: "main-feed" }"#);
    assert!(r.seg_patterns.iter().any(|(n, _)| n == "kind") && r.refused.is_none(), "{r:#?}");
}

/// Rails never matches a String requirement holding a `.` (its router
/// splits segments there), and other punctuation is left to a regex
/// literal: both stay gaps, as does any value that isn't a literal.
#[test]
fn other_plain_text_stays_a_gap() {
    for value in [r#""a.b""#, r#""a+b""#, r#""""#, "FORMATS"] {
        let err = routes(&format!(r#"get "/kinds/:kind", to: "exports#kind", constraints: {{ kind: {value} }}"#))
            .expect_err(value);
        assert!(err.contains("is not a literal regex or a plain name"), "{value}: {err}");
    }
}

#[test]
fn the_ruby_target_matches_plain_names_as_rails_does() {
    ruby_target_answers(format_requirement::literal_overlay(), format_requirement::literal_assertions());
}

#[test]
fn rails_answers_the_plain_name_table() {
    rails_answers(
        format_requirement::LITERAL_CONTROLLER,
        format_requirement::LITERAL_ROUTES,
        format_requirement::literal_assertions(),
    );
}
