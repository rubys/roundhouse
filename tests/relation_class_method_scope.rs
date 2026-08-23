//! A model CLASS METHOD reached through a SCOPE CHAIN takes that
//! relation as its scope, the same way one reached through an
//! association does (`scope_chain::collect_relation_class_method_demand`).
//!
//! Rails runs `User.active.find_by_transfer_id(id)` with the relation as
//! the current scope. The association half of that idea has always been
//! surveyed; the scope half was not, so the call landed on an
//! `ActiveRecord::Relation` — which has no such method — and died on
//! `undefined method`, with the analyzer having said so out loud
//! (`send_dispatch_failed: no known method … on Relation { of: User }`).
//!
//! Two shapes, one registry:
//!
//!   * a body that QUERIES at implicit self roots on `__rel`, so the
//!     chain's conditions actually filter it;
//!   * a body that does NEITHER — no constructor, no query — is
//!     indifferent to the scope, but the send still has to RESOLVE.
//!     It registers with both halves false: the parameter appears
//!     (defaulted, so a direct `Model.x` call is untouched) and the call
//!     site re-roots at the constant.
//!
//! Depth-gated: the receiver must be at least one relation hop deep. A
//! bare `User.some_method` carries no scope, and growing a parameter for
//! it would be pure cost.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.boolean "active", null: false
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  scope :active, -> { where(active: true) }

  def self.find_by_transfer_id(id)
    find_signed(id, purpose: :transfer)
  end

  def self.named(name)
    where(name: name).first
  end

  def self.untouched(id)
    find_signed(id, purpose: :other)
  end
end
"#,
        ),
        (
            "app/controllers/transfers_controller.rb",
            r#"class TransfersController < ApplicationController
  def update
    @user = User.active.find_by_transfer_id(params[:id])
    @named = User.active.named(params[:name])
    @plain = User.untouched(params[:id])
  end
end
"#,
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn emitted(files: Vec<roundhouse::emit::EmittedFile>, suffix: &str) -> String {
    files
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("no emitted file ending in {suffix}"))
}

fn controller() -> String {
    emitted(
        ruby::emit_lowered_controllers(&app()),
        "app/controllers/transfers_controller.rb",
    )
}

fn user_model() -> String {
    emitted(ruby::emit_lowered_models(&app()), "app/models/user.rb")
}

/// A scope-indifferent body still has to be REACHABLE: the send re-roots
/// at the model constant and the relation rides along as `__rel`.
#[test]
fn a_class_method_on_a_scope_chain_re_roots_at_the_constant() {
    let src = controller();
    assert!(
        src.contains("User.find_by_transfer_id(@params[\"id\"], User.active)"),
        "the call re-roots at User and carries the scope chain:\n{src}"
    );
    assert!(
        !src.contains("User.active.find_by_transfer_id"),
        "no class method may survive on a Relation receiver:\n{src}"
    );
}

/// The QUERY half is unchanged by the new channel: a body whose
/// implicit-self `where` Rails would run against the caller's scope
/// roots on `__rel`.
#[test]
fn a_querying_body_roots_on_the_threaded_relation() {
    let src = user_model();
    let named = src.split("def self.named").nth(1).unwrap_or_else(|| {
        panic!("User.named emitted:\n{src}");
    });
    assert!(
        named.contains("__rel.where"),
        "`where(name: name)` runs against the threaded relation:\n{named}"
    );
}

/// A method only ever called on the bare constant grows nothing: the
/// demand channel requires at least one relation hop.
#[test]
fn a_bare_constant_call_grows_no_parameter() {
    let src = user_model();
    let untouched = src.split("def self.untouched").nth(1).unwrap_or_else(|| {
        panic!("User.untouched emitted:\n{src}");
    });
    let sig = untouched.lines().next().unwrap_or("");
    assert!(
        !sig.contains("__rel"),
        "an unscoped-only class method keeps its signature: {sig}"
    );
}
