//! The router's general-regex enforcement (Option D): `constraints:`
//! requirements beyond the digit class are compiled into a small
//! backtrack-free "segment pattern" (`segment_pattern.rs`) that every
//! target's portable matcher checks with plain char comparisons — no
//! `Regexp`, so it lowers even to Elixir, which has no regex-method
//! dispatch at all. A requirement outside the supported subset (a
//! lookahead, a backreference, …) refuses the WHOLE route rather than
//! serve it unenforced.
//!
//! Each test here fails without commit 2 (the runtime enforcement) and
//! passes with it — commit 1 alone only gets the requirement AS FAR AS
//! `FlatRoute.constraints`/`seg_patterns`; nothing checked it at match
//! time, and an unsupported shape wasn't refused at all.
//!
//! `constrained_app`'s slug route also rides a `with_options to:`
//! (upstream #715), confirming this composes: the enclosing
//! `constraints(...)` block's merged requirement still reaches the
//! route and is enforced, same as without `with_options` in between.
//!
//! The Ruby-target recognition case lives here (a plain `unit` CI job
//! can run it); the Spinel one needs a Spinel compiler and lives in
//! `tests/routes_segment_pattern_spinel.rs` instead, run by the
//! `spinel-framework` job.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::App;
use roundhouse::dialect::HttpMethod;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::routes::{FlatRoute, flatten_routes};

fn routes(source: &str) -> Vec<FlatRoute> {
    let mut app = App::default();
    app.routes = ingest_routes(
        format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(),
        "config/routes.rb",
    )
    .expect("ingest routes");
    flatten_routes(&app)
}

fn find<'a>(routes: &'a [FlatRoute], method: HttpMethod, path: &str) -> Option<&'a FlatRoute> {
    routes.iter().find(|r| r.method == method && r.path == path)
}

#[test]
fn compilable_constraint_rides_seg_patterns_not_just_constraints() {
    let r = routes(
        r#"
  get "/@:slug", to: "widgets#show", constraints: { slug: %r{[^@/.]+} }
"#,
    );
    let show = find(&r, HttpMethod::Get, "/@:slug").expect("route present");
    assert_eq!(
        show.seg_patterns,
        vec![("slug".to_string(), "C+103.@/.".to_string())],
        "a compilable requirement must reach the router's own data, not just \
         the roda-converter-facing `constraints` field: {:?}",
        show.seg_patterns
    );
}

#[test]
fn lookahead_constraint_refuses_the_whole_route() {
    let r = routes(
        r#"
  get "/@:slug", to: "widgets#show", constraints: { slug: /(?=foo)bar/ }
"#,
    );
    assert!(
        find(&r, HttpMethod::Get, "/@:slug").is_none(),
        "a lookahead is outside the supported subset; the route must be ABSENT, \
         not served half-enforced: {r:?}"
    );
}

#[test]
fn backreference_constraint_refuses_the_whole_route() {
    let r = routes(
        r#"
  get "/pair/:a", to: "widgets#show", constraints: { a: /(\d)\1/ }
"#,
    );
    assert!(
        find(&r, HttpMethod::Get, "/pair/:a").is_none(),
        "a backreference is outside the supported subset; the route must be \
         ABSENT: {r:?}"
    );
}

#[test]
fn digit_class_constraint_is_unaffected_by_the_new_mechanism() {
    let r = routes(
        r#"
  get "/widgets/:id", to: "widgets#show", constraints: { id: /\d+/ }
"#,
    );
    let show = find(&r, HttpMethod::Get, "/widgets/:id").expect("route present");
    assert_eq!(show.int_params, vec!["id".to_string()]);
    assert!(
        show.seg_patterns.is_empty(),
        "digit-class constraints stay on the existing int_params path: {:?}",
        show.seg_patterns
    );
}

/// The app this section's recognition tests run against — the exact
/// generic shape the bug was found in: a slug route guarded by a block
/// constraint, with a fallback route for anything the guard refuses,
/// plus one route per other required shape. The slug route also rides
/// a `with_options to:` (upstream #715's composition): the merged
/// `constraints(...)` block's requirement has to reach the route the
/// SAME way whether `with_options` sat between it and the route or
/// not — on plain `main` (just #715, no commit 1/2 here) this route is
/// unconstrained either way, so `/@alice@remote.example` wrongly binds
/// it instead of falling through to `show_domain`.
fn constrained_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", r#"
class WidgetsController < ApplicationController
  def show
    render plain: "show:#{params[:slug]}"
  end
  def show_domain
    render plain: "domain:#{params[:slug_with_domain]}"
  end
  def dashed
    render plain: "dashed:#{params[:pair]}"
  end
  def signed
    render plain: "signed:#{params[:n]}"
  end
  def enc
    render plain: "enc:#{params[:value]}"
  end
  def dotted
    render plain: "dotted:#{params[:value]}"
  end
  def dotted2
    render plain: "dotted2:#{params[:value]}"
  end
end
"#)
        .write("config/routes.rb", r#"
Rails.application.routes.draw do
  constraints(slug: %r{[^@/.]+}) do
    with_options to: "widgets#show" do
      get "/@:slug"
    end
  end
  get "/@:slug_with_domain(/*any)", to: "widgets#show_domain"
  get "/dashed/:pair", to: "widgets#dashed", constraints: { pair: /[0-9]+-[0-9]+/ }
  get "/signed/:n", to: "widgets#signed", constraints: { n: /-?\d+/ }
  get "/enc/:value", to: "widgets#enc", constraints: { value: /%40.*/ }
  get "/dotted/:value", to: "widgets#dotted", constraints: { value: %r{[^/]+} }
  get "/dotted2/:value", to: "widgets#dotted2", constraints: { value: %r{([^/])+?} }
end
"#)
}

/// One script, run through both the Ruby and the Spinel target (the
/// shapes it uses — `Main.run_rack`, string interpolation, `raise` —
/// are already proven to compile under Spinel elsewhere in this test
/// suite).
fn recognition_script() -> String {
    r#"
require "stringio"

# A single-assign result + plain index reads (no `a, b = …` multiple
# assignment) — Spinel refuses `MultiWriteNode` outright, matching the
# Python emitter's own MultiAssign gap this suite already measured.
def req(path)
  result = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status = result[0]
  body = result[2].join
  status.to_s + "|" + body
end

# `/@alice` matches the block-constrained slug route — reached through
# a `with_options to:` (upstream #715's composition), so the block's
# merged requirement has to carry through that too.
got = req("/@alice")
raise "slug match: #{got}" unless got == "200|show:alice"

# `/@alice@remote.example` does NOT satisfy `[^@/.]+` (it contains `@`)
# over the WHOLE segment, so it falls through to the next route.
got = req("/@alice@remote.example")
raise "slug fallthrough: #{got}" unless got == "200|domain:alice@remote.example"

# `-?\d+` / digits-dash-digits.
got = req("/signed/-12")
raise "signed -12: #{got}" unless got == "200|signed:-12"
got = req("/signed/12")
raise "signed 12: #{got}" unless got == "200|signed:12"
got = req("/signed/1a")
raise "signed 1a should 404: #{got}" unless got.start_with?("404")

got = req("/dashed/12-34")
raise "dashed 12-34: #{got}" unless got == "200|dashed:12-34"
got = req("/dashed/12-")
raise "dashed 12- should 404: #{got}" unless got.start_with?("404")
got = req("/dashed/-34")
raise "dashed -34 should 404: #{got}" unless got.start_with?("404")

# `%40.*` — literal prefix then any-char star. The constraint runs on
# the RAW (not yet percent-decoded) segment, so `%40` must be literal
# in the request path; the CAPTURED param value reaching the
# controller is percent-decoded as usual (`%40` -> `@`).
got = req("/enc/%40alice")
raise "enc %40alice: #{got}" unless got == "200|enc:@alice"
got = req("/enc/alice")
raise "enc alice should 404: #{got}" unless got.start_with?("404")

# `[^/]+` and `([^/])+?` both accept a dotted value (the lazy
# quantifier is the pattern's only/last item, so it is equivalent to
# the greedy reading under the whole-segment anchor).
got = req("/dotted/a.b.c")
raise "dotted a.b.c: #{got}" unless got == "200|dotted:a.b.c"
got = req("/dotted2/a.b.c")
raise "dotted2 a.b.c: #{got}" unless got == "200|dotted2:a.b.c"

puts "PASS segment-pattern recognition"
"#.to_string()
}

#[test]
fn ruby_target_enforces_every_required_segment_pattern_shape() {
    constrained_app().run_ruby(&recognition_script()).assert_passes();
}

// The Spinel side of this recognition suite lives in its own file,
// `tests/routes_segment_pattern_spinel.rs` — these tests need a Spinel
// compiler, which a plain `unit` CI job doesn't have, so they are
// `#[ignore]`d there and run by the `spinel-framework` job with
// `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`), the same way as
// `not_found_parity_spinel.rs` and its CRuby sibling.
