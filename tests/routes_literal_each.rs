//! Routes drawn inside a loop over a literal Array (rubys/roundhouse#999).
//!
//! `%w[alpha beta].each { |page| get page, to: "pages##{page}" }` is a
//! call with a receiver, and the walker skipped every receiver-bearing
//! call (the guard is there for a nested `routes.draw`), block and all,
//! with no gap. Rails evaluates `routes.rb` as Ruby and draws one route
//! per element. A literal Array of Strings or Symbols is now unrolled,
//! the block parameter bound in the route evaluator's frame; any other
//! receiver-bearing call with a block is a located `Unsupported` gap.
//!
//! The expected answers are Rails 8.1.4's on the same app, booted
//! in-process with `Rack::MockRequest`.

use std::collections::HashMap;

use roundhouse::ingest::routes::ingest_routes_with_draws;
use roundhouse::ingest::{survey, IngestError};
use roundhouse::lower::flatten_routes;
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn ingest(source: &str) -> Result<roundhouse::dialect::RouteTable, IngestError> {
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source.as_bytes(), "config/routes.rb", &HashMap::new())
    });
    result
}

/// `VERB /path controller#action` for every flattened route, in order.
fn routes_of(source: &str) -> Vec<String> {
    let mut app = App::default();
    app.routes = ingest(source).expect("strict ingest accepts the loop");
    flatten_routes(&app)
        .into_iter()
        .map(|r| format!("{:?} {} {}#{}", r.method, r.path, r.controller.0, r.action))
        .collect()
}

fn survey_ingest(source: &str) -> (Vec<String>, Vec<String>) {
    survey::activate();
    let result = ingest(source);
    let gaps = survey::drain();
    let mut app = App::default();
    app.routes = result.expect("survey mode recovers the sibling routes");
    let paths = flatten_routes(&app).into_iter().map(|r| r.path).collect();
    (paths, gaps.iter().map(|g| g.to_string()).collect())
}

fn draw(body: &str) -> String {
    format!("Rails.application.routes.draw do\n{body}end\n")
}

#[test]
fn a_percent_w_loop_draws_one_route_per_element() {
    let routes = routes_of(&draw("  %w[alpha beta].each { |page| get page, to: \"pages##{page}\" }\n"));
    assert_eq!(
        routes,
        vec!["Get /alpha PagesController#alpha", "Get /beta PagesController#beta"],
    );
}

#[test]
fn an_interpolated_path_reads_the_loop_variable() {
    let routes =
        routes_of(&draw("  %w[alpha beta].each do |page|\n    get \"docs/#{page}\", to: \"pages##{page}\"\n  end\n"));
    assert_eq!(
        routes,
        vec!["Get /docs/alpha PagesController#alpha", "Get /docs/beta PagesController#beta"],
    );
}

#[test]
fn symbol_arrays_draw_resources_with_their_options() {
    let percent_i = routes_of(&draw("  %i[widgets gadgets].each { |name| resources name, only: :index }\n"));
    let brackets =
        routes_of(&draw("  [:widgets, :gadgets].each do |name|\n    resources name, only: :index\n  end\n"));
    let expected = vec!["Get /widgets WidgetsController#index", "Get /gadgets GadgetsController#index"];
    assert_eq!(percent_i, expected);
    assert_eq!(brackets, expected);

    let with_path = routes_of(&draw("  %i[widgets].each { |n| resources n, only: [:show], path: \"w\" }\n"));
    assert_eq!(with_path, vec!["Get /w/:id WidgetsController#show"]);
}

#[test]
fn a_loop_inside_a_namespace_keeps_the_namespace() {
    let routes = routes_of(&draw(
        "  namespace :admin do\n    %w[alpha beta].each { |page| get page, to: \"pages##{page}\" }\n  end\n",
    ));
    assert_eq!(
        routes,
        vec![
            "Get /admin/alpha Admin::PagesController#alpha",
            "Get /admin/beta Admin::PagesController#beta",
        ],
    );
}

/// The loop's frame copies the concern's, so `options` is still bound
/// inside the block.
#[test]
fn a_loop_inside_a_concern_still_sees_the_concern_options() {
    let routes = routes_of(&draw(
        "  concern :paged do |options|\n    %w[alpha beta].each { |page| get \"#{options[:prefix]}#{page}\", to: \"pages##{page}\" }\n  end\n  concerns :paged, prefix: \"p_\"\n",
    ));
    assert_eq!(
        routes,
        vec!["Get /p_alpha PagesController#alpha", "Get /p_beta PagesController#beta"],
    );
}

/// A collection the walker cannot read (a constant, a computed value) or
/// a block method other than `each` is a gap with a line, not a silent
/// skip. Strict ingest fails; survey keeps the sibling routes.
#[test]
fn any_other_receiver_block_is_a_located_gap() {
    for (body, line) in [
        ("  PAGES.each { |page| get page, to: \"pages##{page}\" }\n", 3),
        ("  %w[alpha beta].each_with_index { |page, i| get page, to: \"pages##{page}\" }\n", 3),
        ("  2.times { |i| get \"p#{i}\", to: \"pages#show\" }\n", 3),
        ("  [page_name].each { |page| get page, to: \"pages##{page}\" }\n", 3),
    ] {
        let source = draw(&format!("  get \"plain\", to: \"pages#plain\"\n{body}"));
        assert!(ingest(&source).is_err(), "strict ingest fails loud: {source}");
        let (paths, gaps) = survey_ingest(&source);
        assert_eq!(paths, vec!["/plain"], "the sibling route survives: {source}");
        assert!(
            gaps.iter().any(|m| m.contains("unsupported routes DSL") && m.contains(&format!("line {line}"))),
            "the loop is ledgered with its line: {gaps:?}"
        );
    }
}

/// The issue's fixture, emitted and run. Every route but `/plain` sits
/// inside a loop; an all-looped draw block must still emit a RouteTable.
#[test]
fn looped_routes_answer_as_in_rails() {
    let looped = r#"Rails.application.routes.draw do
  %w[alpha beta].each { |page| get page, to: "pages##{page}" }
  [:widgets, :gadgets].each do |name|
    resources name, only: :index
  end
  get "plain", to: "pages#plain"
end
"#;
    let all_looped = "Rails.application.routes.draw do\n  %w[alpha beta plain].each { |page| get page, to: \"pages##{page}\" }\nend\n";
    let cases: [(&str, &[(&str, &str)]); 2] = [
        (
            looped,
            &[
                ("/alpha", "alpha"),
                ("/beta", "beta"),
                ("/widgets", "widgets"),
                ("/gadgets", "gadgets"),
                ("/plain", "plain"),
            ],
        ),
        (all_looped, &[("/alpha", "alpha"), ("/beta", "beta"), ("/plain", "plain")]),
    ];
    for (routes, expected) in cases {
        let checks: String = expected
            .iter()
            .map(|(path, body)| format!("check.({path:?}, {body:?})\n"))
            .collect();
        emit_and_run::empty_app()
            .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
            .write(
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::API\nend\n",
            )
            .write(
                "app/controllers/pages_controller.rb",
                "class PagesController < ApplicationController\n  def alpha = render(plain: \"alpha\")\n  def beta = render(plain: \"beta\")\n  def plain = render(plain: \"plain\")\nend\n",
            )
            .write(
                "app/controllers/widgets_controller.rb",
                "class WidgetsController < ApplicationController\n  def index = render(plain: \"widgets\")\nend\n",
            )
            .write(
                "app/controllers/gadgets_controller.rb",
                "class GadgetsController < ApplicationController\n  def index = render(plain: \"gadgets\")\nend\n",
            )
            .write("config/routes.rb", routes)
            .run_ruby(&format!(
                r##"
require "stringio"
check = lambda do |path, expected|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each {{ |part| text << part }}
  raise "#{{path}}: expected 200 #{{expected.inspect}}, got #{{status}} #{{text.inspect}}" unless status == 200 && text == expected
end
{checks}puts "ALL OK"
"##
            ))
            .assert_passes();
    }
}

/// A `mount` of a real literal-source engine, written once inside a
/// literal-each loop (rubys/roundhouse#1027, CodeRabbit on #1027).
/// `ingest_literal_each` walks the loop body with `ingest_route_body`
/// directly, without raising `mount_scope_depth` the way every other
/// block walker does (`if`/`unless` arms, `constraints`/`member`/
/// `collection`/`devise_scope`, and the generic `block_entries` for
/// `namespace`/`scope`/concern blocks all increment it before
/// recursing). So a `mount` inside the loop is read as if it were
/// top-level: on a 2+-element loop the first iteration's mount
/// succeeds and claims the engine in `mounted_engines`, and the second
/// then hits the check meant for two *separate* top-level `mount`
/// statements of the same engine, reporting "mounting the same engine
/// class more than once" — even though this is one `mount` call,
/// executed twice by the loop. Rails would not boot this app either
/// (both mounts get the same default route name, from the same engine
/// class, since this feature does not compose `as:`), but it fails
/// the same way on every iteration, not after silently serving the
/// first one. Matching that, and matching every sibling block walker,
/// a `mount` inside the loop is a located gap on every iteration, not
/// a partial mount.
#[test]
fn a_mount_inside_a_literal_each_loop_is_a_located_gap_on_every_iteration() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse-routes-literal-each-mount-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let app_dir = root.join("app");
    for (path, source) in [
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ActionController::Base\n  def index\n    render plain: \"widgets\"\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
        (
            "Gemfile.lock",
            "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n",
        ),
        (
            "vendor/catalog/lib/catalog/engine.rb",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n",
        ),
        (
            "vendor/catalog/app/controllers/catalog/products_controller.rb",
            "module Catalog\n  class ProductsController < ActionController::Base\n    def index\n      render plain: \"products\"\n    end\n  end\nend\n",
        ),
        (
            "vendor/catalog/config/routes.rb",
            "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  %w[a b].each { |x| mount Catalog::Engine, at: \"/catalog\" }\nend\n",
        ),
    ] {
        let file = app_dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }

    let (built, _) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&app_dir));
    let _ = std::fs::remove_dir_all(&root);
    let app = built.expect("ingest the app (mount gaps are diagnostics, not IngestErrors)");

    let gaps: Vec<&str> = app.routes.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        gaps.iter().filter(|m| m.contains("only top-level literal engine mounts are composed")).count(),
        2,
        "every iteration's mount is a located gap, not just a second/duplicate one: {gaps:?}"
    );
    assert!(
        !gaps.iter().any(|m| m.contains("mounting the same engine class more than once")),
        "the loop's mount must not be read as two separate top-level mounts: {gaps:?}"
    );

    let routes = flatten_routes(&app);
    assert!(
        routes.iter().any(|r| r.path == "/widgets"),
        "the unlooped sibling route survives: {routes:?}"
    );
    assert!(
        !routes.iter().any(|r| r.path.starts_with("/catalog")),
        "neither loop iteration's mount should partially succeed: {routes:?}"
    );
}
