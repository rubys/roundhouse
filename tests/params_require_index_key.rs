//! `params.require(:user)[:role]` — an INDEX on the required sub-hash,
//! and the `presence_in` that reads it.
//!
//! Rails' `require` answers an `ActionController::Parameters`, whose
//! access is indifferent. `@params` is a plain String-keyed Hash, so a
//! Symbol key finds nothing and the read answers **nil**. That is a
//! silent wrong answer, not an error — campfire's
//!
//! ```ruby
//! { role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member" }
//! ```
//!
//! would have quietly demoted every role change to the default had the
//! `||` come first. What it did instead was die on `presence_in` for
//! nil, one method later — which is also a gap: `Object#presence_in` is
//! an ActiveSupport core_ext reopen only the CRuby overlay could host,
//! so every other target had an unresolved call.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_presence_in_grounding;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
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
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def update
    @user = User.find(params[:id])
    @user.update(role_params)
  end

  private
    def role_params
      { role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member" }
    end
end
"#,
        ),
    ]))
    .expect("ingest params-index app")
}

fn controller_src() -> String {
    // The grounding is a POST-ANALYZE pass, so the harness has to run
    // the analyzer and the pass — `emit_lowered_controllers` alone
    // reads an App that never went through the pipeline.
    let mut app = app();
    Analyzer::new(&app).analyze(&mut app);
    apply_presence_in_grounding(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("users_controller.rb"))
        .map(|f| f.content.clone())
        .expect("users_controller.rb")
}

/// The index key becomes a String, and the `require` under it still
/// lowers — the two have to happen in one step, because the index is
/// the OUTER node and the rewriter does not recurse into what it
/// replaces.
#[test]
fn the_index_key_on_a_required_sub_hash_is_a_string() {
    let src = controller_src();
    assert!(
        src.contains(r#"Params.require_key(@params, "user")["role"]"#),
        "the symbol index must lower to a string key:\n{src}"
    );
    assert!(
        !src.contains("[:role]"),
        "no symbol key may survive into the emit:\n{src}"
    );
}

/// `presence_in` is grounded on `ActiveSupport`, the same home its
/// `presence` sibling uses, so every target has a method to dispatch.
#[test]
fn presence_in_grounds_on_active_support() {
    let src = controller_src();
    assert!(
        src.contains("ActiveSupport.presence_in("),
        "presence_in must ground:\n{src}"
    );
    // …and no site keeps the receiver form. `ActiveSupport.` is the
    // only receiver this method may have after the pass.
    assert_eq!(
        src.matches(".presence_in(").count(),
        src.matches("ActiveSupport.presence_in(").count(),
        "no receiver-form presence_in may survive:\n{src}"
    );
}
