//! A Hash under a route helper's query option renders as Rails'
//! `Hash#to_query` does: one `key[sub]=value` pair per entry, sorted,
//! with a nil value as a bare `key[sub]` and an empty Hash left out.
//! The helper wrote the Hash's `inspect` as a single value, and its
//! signature declared the key a String.
//!
//! Expected paths are Rails 8.1.4's for the same routes and calls
//! (actionpack 8.1.4, oracle `rails-oracle.rb`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ROUTES: &str = "Rails.application.routes.draw do\n  namespace :admin do\n    resources :domain_allows, only: [:create]\n  end\n  resources :widgets, only: [:index]\n  resources :tags, only: [:index]\nend\n";

// The call sites that give the helpers their keys.
const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def index
    render plain: [
      admin_domain_allows_path(domain_allow: { domain: params[:domain] }),
      widgets_path(filter: { kind: params[:kind] }, page: params[:page])
    ].join(" ")
  end
end
"#;

// A variable inner value: its Array needs `to_query`'s recursion, so the
// key keeps the scalar form. Only the sidecar test carries it: as a
// scalar key it is the call Spinel refused before this change.
const TAGS_CONTROLLER: &str = "class TagsController < ApplicationController\n  def index\n    names = [\"a\", \"b\"]\n    render plain: tags_path(filter: { names: names })\n  end\nend\n";

const ASSERTIONS: &str = r#"
require_relative "app/route_helpers"
got = [
  RouteHelpers.admin_domain_allows_path(domain_allow: { domain: "example.com" }),
  RouteHelpers.admin_domain_allows_path(domain_allow: { domain: "a b&c" }),
  RouteHelpers.admin_domain_allows_path(domain_allow: { note: "x", domain: "example.com" }),
  RouteHelpers.admin_domain_allows_path(domain_allow: { domain: nil }),
  RouteHelpers.admin_domain_allows_path(domain_allow: { limit: 5 }),
  RouteHelpers.admin_domain_allows_path(domain_allow: {}),
  RouteHelpers.widgets_path(filter: { kind: "x" }, page: "2"),
  RouteHelpers.widgets_path(filter: {}, page: "2")
]
want = [
  "/admin/domain_allows?domain_allow%5Bdomain%5D=example.com",
  "/admin/domain_allows?domain_allow%5Bdomain%5D=a+b%26c",
  "/admin/domain_allows?domain_allow%5Bdomain%5D=example.com&domain_allow%5Bnote%5D=x",
  "/admin/domain_allows?domain_allow%5Bdomain%5D",
  "/admin/domain_allows?domain_allow%5Blimit%5D=5",
  "/admin/domain_allows",
  "/widgets?filter%5Bkind%5D=x&page=2",
  "/widgets?page=2"
]
raise got.zip(want).reject { |g, w| g == w }.inspect unless got == want
puts "hash query passed"
"#;

// A literal `nil` for the Hash keyword. Not in the Spinel run: under
// `--rbs .`, Spinel's C for a `nil` passed to a `Hash[Symbol, untyped]?`
// keyword does not compile (matz/spinel#10677).
const NIL_OPTION: &str = r#"
got = RouteHelpers.admin_domain_allows_path(domain_allow: nil)
raise got.inspect unless got == "/admin/domain_allows"
puts "nil option passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("config/routes.rb", ROUTES)
}

#[test]
fn a_hash_query_option_is_declared_a_hash() {
    let (emitted, errors) = example().write("app/controllers/tags_controller.rb", TAGS_CONTROLLER).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let sig = std::fs::read_to_string(emitted.join("app/route_helpers.rbs")).expect("read route_helpers.rbs");
    for want in [
        "def self.admin_domain_allows_path: (?domain_allow: Hash[Symbol, untyped]?) -> String",
        "def self.widgets_path: (?filter: Hash[Symbol, untyped]?, ?page: String?) -> String",
        "def self.tags_path: (?filter: String?) -> String",
    ] {
        assert!(sig.contains(want), "no `{want}` in:\n{sig}");
    }
}

#[test]
fn a_hash_query_option_renders_as_rails_does_on_ruby() {
    let run = example().run_ruby(&format!("{ASSERTIONS}{NIL_OPTION}"));
    run.assert_passes();
    assert!(run.stdout.contains("hash query passed"), "{}", run.stdout);
    assert!(run.stdout.contains("nil option passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_hash_query_option_renders_as_rails_does_on_spinel_with_rbs() {
    let run = example().run_spinel_with_rbs(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("hash query passed"), "{}", run.stdout);
}

/// The Python emit of the same helpers, run under `python3`.
#[test]
#[ignore = "requires python3"]
fn a_hash_query_option_renders_as_rails_does_on_python() {
    let (emitted, errors) = example().emit(BuildTarget::Python);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(
        emitted.join("probe.py"),
        r#"from app.v2.view_helpers import ViewHelpers
import app.v2.route_helpers as route_helpers
route_helpers.ViewHelpers = ViewHelpers
from app.v2.route_helpers import RouteHelpers

got = [
    RouteHelpers.admin_domain_allows_path({"domain": "example.com"}),
    RouteHelpers.admin_domain_allows_path({"note": "x", "domain": "example.com"}),
    RouteHelpers.admin_domain_allows_path({"domain": None}),
    RouteHelpers.admin_domain_allows_path({}),
    RouteHelpers.admin_domain_allows_path(None),
    RouteHelpers.widgets_path({"kind": "x"}, "2"),
]
want = [
    "/admin/domain_allows?domain_allow%5Bdomain%5D=example.com",
    "/admin/domain_allows?domain_allow%5Bdomain%5D=example.com&domain_allow%5Bnote%5D=x",
    "/admin/domain_allows?domain_allow%5Bdomain%5D",
    "/admin/domain_allows",
    "/admin/domain_allows",
    "/widgets?filter%5Bkind%5D=x&page=2",
]
if got != want:
    raise AssertionError(f"{got!r}")
print("python hash query passed")
"#,
    )
    .expect("write probe");
    let output = std::process::Command::new("python3").arg("probe.py").current_dir(&emitted).output().expect("run python3");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("python hash query passed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
