//! The Spinel side of the segment-pattern recognition suite
//! (`tests/routes_segment_pattern_matcher.rs` has the Ruby-target case
//! and the FlatRoute-level ones). These need a Spinel compiler, which
//! a plain `unit` CI job doesn't have, so they are `#[ignore]`d like
//! their sibling suites (`not_found_parity_spinel.rs`,
//! `pessimistic_locking.rs`); CI's `spinel-framework` job runs them
//! with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The app this suite runs against — the exact generic shape the bug
/// was found in: a slug route guarded by a block constraint (reached
/// through a `with_options to:`, upstream #715's composition), with a
/// fallback route for anything the guard refuses, plus one route per
/// other required shape. Duplicated from the Ruby-target file rather
/// than shared — `tests/*.rs` are separate compilation units, and
/// `not_found_parity_spinel.rs` keeps its own fixture builder the
/// same way.
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

/// Direct `Router.match_pattern` calls (not a full HTTP dispatch —
/// `run_spinel`'s plain `boot` prelude doesn't load the app's
/// `Main`/Rack entry point, only the runtime library classes, the
/// same reason `route_path_decoding.rs`'s own Spinel byte-contract
/// test calls `Router.match_pattern` directly rather than dispatching
/// a request). The encoded pattern strings are the exact ones
/// `segment_pattern.rs` produces for each shape (checked against this
/// in its own Rust unit tests and against the Ruby target in the
/// sibling file).
#[test]
#[ignore = "requires the Spinel toolchain"]
fn spinel_target_enforces_every_required_segment_pattern_shape() {
    let script = r#"
R = ActionDispatch::Router

def check(pattern, path, seg_constraints, expect_match)
  hit = R.match_pattern(pattern, path, "", seg_constraints)
  matched = !hit.nil?
  raise "pattern=#{pattern} path=#{path} matched=#{matched} expected=#{expect_match}" unless matched == expect_match
end

slug_pat = "4.slug9.C+103.@/."
check("/@:slug", "/@alice", slug_pat, true)
check("/@:slug", "/@alice@remote.example", slug_pat, false)

signed_pat = "1.n24.L?001.-C+0010.0123456789"
check("/signed/:n", "/signed/-12", signed_pat, true)
check("/signed/:n", "/signed/12", signed_pat, true)
check("/signed/:n", "/signed/1a", signed_pat, false)

dashed_pat = "4.pair41.C+0010.0123456789L1001.-C+0010.0123456789"
check("/dashed/:pair", "/dashed/12-34", dashed_pat, true)
check("/dashed/:pair", "/dashed/12-", dashed_pat, false)
check("/dashed/:pair", "/dashed/-34", dashed_pat, false)

enc_pat = "5.value15.L1003.%40C*010."
check("/enc/:value", "/enc/%40alice", enc_pat, true)
check("/enc/:value", "/enc/alice", enc_pat, false)

dotted_pat = "5.value7.C+101./"
check("/dotted/:value", "/dotted/a.b.c", dotted_pat, true)

puts "PASS Spinel segment-pattern recognition"
"#;
    constrained_app().run_spinel(script).assert_passes();
}
