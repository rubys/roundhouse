//! `params.fetch(:user_ids, []).including(x)` — the last strict-emit
//! error campfire carried.
//!
//! Two halves, and both are needed:
//!
//! 1. `Hash#fetch(k, default)` answers `default` when the key is
//!    missing, so the result is `value | typeof(default)`. The params
//!    model calls every value a `Str` (`Roundhouse::ParamValue` is the
//!    runtime type, not one the analyzer carries), so the `[]` literal
//!    is the ONLY evidence about this key's shape.
//! 2. ActiveSupport's `Enumerable#including` lowers to
//!    `<recv>.to_a + [args]`, and that pass stamps `to_a`'s type off
//!    the receiver. A union receiver has to answer from whichever arm
//!    can — the same policy the body-typer's union dispatch uses.
//!
//! The gate lowers and THEN diagnoses, which is the order the CLI runs
//! them in and the only one in which the defect exists: the `to_a` is
//! SYNTHESIZED by a lowering, so an analyze-only gate cannot see it.
//! `analyze_and_lower`'s own return value is not enough either — it
//! carries the post-analyze RESIDUE diagnostics, not `diagnose`'s. A
//! first draft of these tests asserted against that list alone and
//! passed under both ablations.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn diagnostics_for(action: &str) -> Vec<String> {
    // campfire's own shape: the action reaches the params read through
    // two private methods. Inlining it into the action body does NOT
    // reproduce the failure — the dispatch resolves there — so the
    // fixture keeps the hop.
    let controller = format!(
        r#"class UsersController < ApplicationController
  def index
    @users = selected_users
  end

  private
    def selected_users
      User.where(id: {action})
    end

    def selected_users_ids
      params.fetch(:user_ids, [])
    end
end
"#
    );
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/users_controller.rb", &controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/users\", to: \"users#index\"\nend\n",
        ),
    ]))
    .expect("ingest");
    // Mirrors the CLI: lower first, THEN diagnose. `analyze_and_lower`
    // returns only the post-analyze residue, and the `to_a` under test
    // does not exist until the `including` pass has run.
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect()
}

#[test]
fn an_array_default_gives_fetch_its_shape() {
    // Without the default's type, `fetch` answered `Str | Nil` and the
    // Array-only call failed dispatch on it.
    let diags = diagnostics_for("selected_users_ids.map { |i| i }");
    assert!(
        !diags.iter().any(|d| d.contains("send_dispatch_failed") && d.contains("`map`")),
        "`fetch(:k, [])` should answer an Array arm that `map` resolves \
         against; diagnostics = {diags:?}"
    );
}

#[test]
fn including_reads_to_a_off_the_answering_union_arm() {
    // The whole campfire shape. `including` lowers to `.to_a + [...]`,
    // and the receiver is `Array[…] | Str`: the Array arm answers
    // `to_a`, the Str arm declines, and leaving the union unstamped put
    // a `no known method to_a` error on working code (the CRuby lane's
    // rooms_directs_controller_test is green on it).
    let diags = diagnostics_for("selected_users_ids.including(1)");
    assert!(
        !diags.iter().any(|d| d.contains("send_dispatch_failed") && d.contains("`to_a`")),
        "the lowered `to_a` should take its type from the union's Array \
         arm; diagnostics = {diags:?}"
    );
}
