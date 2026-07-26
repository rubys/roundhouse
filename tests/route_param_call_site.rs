//! A record handed to a path helper becomes its slug at the CALL SITE
//! (`emit::ruby::library::apply_route_param_lowering`), not inside the
//! helper.
//!
//! Rails puts `to_param` in the helper body, where the param is
//! untyped so a record arrives whole. Our segments are declared
//! `String`, and under AOT that promise is kept: a record handed to a
//! `String` slot is coerced on the way in, so a helper-side `to_param`
//! saw a String that had already lost its identity — lobsters'
//! `tag_path(tag)` rendered `/t/` with an empty segment, and then
//! raised `undefined method 'to_param' for an instance of String` on
//! top of it (31 of 114 visits on the spinel lane, 2026-07-26).
//!
//! The rule is positive-signal-only: wrap what is provably a record,
//! leave everything else. Scalars already ARE slugs, and this corpus
//! passes plenty of them (`tag.id`, `story.short_id`, `user.username`).
//!
//! The fixtures below call `RouteHelpers.<x>_path` directly, which is
//! the shape the pass contracts on — the upstream rewrites that
//! PRODUCE those calls (bare template helpers, the `url_helpers`
//! collapse, `emit_url_arg`) have their own tests, and the end-to-end
//! wiring is held by the lobsters byte-parity lane.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(extra_model_body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    t.string :username\n  end\n  \
             create_table :stories do |t|\n    t.string :short_id\n    t.integer :user_id\n  end\nend\n"
                .to_string(),
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def to_param\n    username\n  end\nend\n".to_string(),
        ),
        (
            "app/models/story.rb",
            format!(
                "class Story < ApplicationRecord\n  belongs_to :user\n{extra_model_body}end\n"
            ),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\n  resources :stories\nend\n"
                .to_string(),
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    ruby::emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn singular_association_read_gets_to_param() {
    let out = emit("  def author_link\n    RouteHelpers.user_path(self.user)\n  end\n");
    assert!(
        out.contains("RouteHelpers.user_path(self.user.to_param)"),
        "a belongs_to read whose target overrides to_param is a record:\n{out}",
    );
}

#[test]
fn scalar_reader_is_left_alone() {
    // `username` is a column, not an association — it already IS the
    // slug, and wrapping it would call to_param on a String.
    let out = emit("  def author_link\n    RouteHelpers.user_path(self.user.username)\n  end\n");
    assert!(
        out.contains("RouteHelpers.user_path(self.user.username)"),
        "a column read must not be wrapped:\n{out}",
    );
    assert!(
        !out.contains("username.to_param"),
        "a column read must not be wrapped:\n{out}",
    );
}

#[test]
fn bare_name_matching_a_model_gets_to_param() {
    // The convention the view lowerer's `ivar_ty` already commits to:
    // a local named for a model holds one. This is what reaches
    // lobsters' `tag_path(tag)` inside `ms.tags.each do |tag|`, where
    // the view pipeline leaves the arg's type an unresolved `Ty::Var`.
    let out = emit("  def author_link(user)\n    RouteHelpers.user_path(user)\n  end\n");
    assert!(
        out.contains("RouteHelpers.user_path(user.to_param)"),
        "a bare name that IS a model name holds a record:\n{out}",
    );
}

#[test]
fn an_already_written_to_param_is_not_doubled() {
    let out = emit("  def author_link\n    RouteHelpers.user_path(self.user.to_param)\n  end\n");
    assert!(
        !out.contains("to_param.to_param"),
        "to_param must not stack:\n{out}",
    );
}

#[test]
fn helper_bodies_keep_bare_segments() {
    // The conversion lives at the call site now; the helper interpolates
    // whatever slug it was handed.
    let files = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :username\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def to_param\n    username\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let helpers = ruby::emit_lowered_routes(&app);
    let src = ruby::emit_library(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let _ = (helpers, src);
    let funcs = roundhouse::lower::lower_routes_to_library_functions(&app);
    let bodies: Vec<String> = funcs.iter().map(|f| ruby::emit_expr(&f.body)).collect();
    assert!(
        bodies.iter().any(|b| b.contains("#{id}")),
        "helper segments interpolate the param bare: {bodies:?}",
    );
    assert!(
        !bodies.iter().any(|b| b.contains("to_param")),
        "no helper body calls to_param: {bodies:?}",
    );
}
