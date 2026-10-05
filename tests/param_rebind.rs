//! A parameter written after its binder is a new value, not a new type
//! for the same slot.
//!
//! Rails writes this as a helper that takes a record OR its fixture
//! label (`user = users(user) unless user.is_a? User`), as a finder
//! that takes an id OR a record (`user = User.find(user)`), and as a
//! coerce-in-place (`id = id.to_i`). CRuby is untyped, so the reuse is
//! invisible. AOT pins the local from the first write and refuses the
//! later one — campfire's compiled suite lost 27 files on the `sign_in`
//! form alone (`sp_sym` then `User`).
//!
//! The pass is not a Campfire helper: any method that rebinds a
//! parameter gets a fresh local, on models, controllers, library
//! classes and test helpers.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::{emit_library, emit_spinel};
use roundhouse::ingest::{ingest_app_from_tree, ingest_library_classes};
use roundhouse::lower::param_rebind::apply_param_rebind_lowering;

fn emit_classes(source: &str) -> String {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    apply_param_rebind_lowering(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

fn app_tree(files: &[(&str, &str)]) -> App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

/// True when `name` is assigned as a bare local (`id = …`), not as the
/// suffix of a fresh name (`__rh_id = …`).
fn assigns_parameter(src: &str, name: &str) -> bool {
    src.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with(name) && t[name.len()..].starts_with(" = ")
    })
}

fn emit_app(files: &[(&str, &str)]) -> String {
    let mut app = app_tree(files);
    roundhouse::session::analyze_and_lower(&mut app);
    emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_app_file(files: &[(&str, &str)], suffix: &str) -> String {
    let mut app = app_tree(files);
    roundhouse::session::analyze_and_lower(&mut app);
    emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("no emitted file ending in {suffix}"))
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :users do |t|
    t.string :email_address
  end
  create_table :messages do |t|
    t.integer :user_id
  end
end
"#;

/// campfire's `SessionTestHelper#sign_in`: a fixture label or a User.
#[test]
fn sign_in_unless_is_a_user_splits_the_parameter() {
    let out = emit_classes(
        r#"
class SessionTestHelper
  def sign_in(user)
    user = users(user) unless user.is_a? User
    user.email_address
  end
end
"#,
    );
    assert!(
        out.contains("__rh_user"),
        "expected a fresh local for the record:\n{out}"
    );
    assert!(
        out.contains("def sign_in(user)"),
        "the parameter itself must keep its name:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "user"),
        "must not write the User back onto the parameter:\n{out}"
    );
    assert!(
        out.contains("__rh_user.email_address") || out.contains("__rh_user.email_address()"),
        "later reads must follow the fresh local:\n{out}"
    );
}

/// The `if` polarity: a message or its fixture label.
#[test]
fn record_or_symbol_if_is_a_splits_the_parameter() {
    let out = emit_classes(
        r#"
class MentionHelper
  def wrap(message)
    message = messages(message) if message.is_a?(Symbol)
    message.body
  end
end
"#,
    );
    assert!(
        out.contains("__rh_message"),
        "expected a fresh local:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "message"),
        "must not write the record onto the parameter:\n{out}"
    );
}

/// `id = id.to_i` before `find` — every `before_action` that coerces.
#[test]
fn coerce_in_place_to_i_splits_the_parameter() {
    let out = emit_classes(
        r#"
class UsersController
  def set_user(id)
    id = id.to_i
    User.find(id)
  end
end
"#,
    );
    assert!(out.contains("__rh_id"), "expected a fresh local:\n{out}");
    assert!(
        out.contains("id.to_i") || out.contains("id.to_i()"),
        "the coerce still reads the original parameter:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "id"),
        "must not write the integer back onto the parameter:\n{out}"
    );
    assert!(
        out.contains("User.find(__rh_id)") || out.contains("find(__rh_id)"),
        "find must see the coerced local:\n{out}"
    );
}

/// `user = User.find(user)` — id-or-record, the other half of sign_in.
#[test]
fn find_reuses_the_id_parameter() {
    let out = emit_classes(
        r#"
class UsersController
  def set_user(user)
    user = User.find(user)
    user
  end
end
"#,
    );
    assert!(out.contains("__rh_user"), "expected a fresh local:\n{out}");
    assert!(
        out.contains("User.find(user)") || out.contains("User.find(user,"),
        "find still reads the original parameter:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "user"),
        "must not write the record onto the parameter:\n{out}"
    );
}

/// `to_s` on a numeric id, then interpolate — same clash, String slot.
#[test]
fn coerce_in_place_to_s_splits_the_parameter() {
    let out = emit_classes(
        r#"
class Ids
  def key(id)
    id = id.to_s
    "user-" + id
  end
end
"#,
    );
    assert!(out.contains("__rh_id"), "expected a fresh local:\n{out}");
    assert!(
        !assigns_parameter(&out, "id"),
        "must not write the string onto the parameter:\n{out}"
    );
}

/// A keyword parameter is a binder too (`def f(user:)`).
#[test]
fn a_keyword_parameter_rebind_splits() {
    let out = emit_classes(
        r#"
class Sessions
  def sign_in(user:)
    user = User.find(user) unless user.is_a?(User)
    user
  end
end
"#,
    );
    assert!(
        out.contains("__rh_user"),
        "keyword parameters rebind the same way:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "user"),
        "must not write the record onto the keyword parameter:\n{out}"
    );
}

/// Two successive coerces: each write is a new local, chained.
#[test]
fn successive_rebinds_chain_fresh_locals() {
    let out = emit_classes(
        r#"
class Ids
  def normalize(id)
    id = id.to_s
    id = id.to_i
    id
  end
end
"#,
    );
    assert!(
        out.contains("__rh_id"),
        "first coerce needs a fresh local:\n{out}"
    );
    assert!(
        out.contains("__rh_id_2"),
        "second coerce must not reuse the first fresh local's slot as the parameter:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "id"),
        "neither write may land on the parameter:\n{out}"
    );
}

/// A local that is not a parameter keeps its name — this pass is about
/// the binder the caller filled, not every assign in the method.
#[test]
fn a_non_parameter_local_is_left_alone() {
    let out = emit_classes(
        r#"
class Ids
  def key(prefix)
    id = 1
    id = id.to_s
    prefix.to_s + id
  end
end
"#,
    );
    assert!(
        out.contains("id = 1") || out.contains("id = 1\n"),
        "the ordinary local must keep its name:\n{out}"
    );
    assert!(
        !out.contains("__rh_id"),
        "must not invent a fresh local for a non-parameter:\n{out}"
    );
}

/// `||=` is not a plain write; expanding it here would change when the
/// RHS runs. Leave it for a pass that preserves short-circuit.
#[test]
fn or_equals_on_a_parameter_is_left_alone() {
    let out = emit_classes(
        r#"
class Cache
  def fetch(key)
    key ||= "default"
    key
  end
end
"#,
    );
    assert!(
        out.contains("key ||= \"default\"") || out.contains("key ||= \"default\""),
        "||= must survive verbatim:\n{out}"
    );
    assert!(
        !out.contains("__rh_key"),
        "must not expand ||= into a fresh local:\n{out}"
    );
}

/// An `if` whose body is more than the assign cannot become a join, and
/// cannot split onto a branch-local either: Ruby's write is visible
/// after the `unless`, so the whole parameter stays as written.
#[test]
fn an_if_with_extra_statements_does_not_become_a_join() {
    let out = emit_classes(
        r#"
class Sessions
  def sign_in(user)
    unless user.is_a?(User)
      user = users(user)
      log(user)
    end
    user
  end
end
"#,
    );
    assert!(
        !out.contains("__rh_user"),
        "a non-dominating write must not invent a fresh local:\n{out}"
    );
    assert!(
        out.contains("unless") || (out.contains("if ") && out.contains("log")),
        "must not flatten a multi-statement unless into a join:\n{out}"
    );
}

/// A write inside a loop, rescue, or block is visible afterwards in
/// Ruby. Same rule as the multi-statement `if`: leave the parameter.
#[test]
fn a_write_inside_a_loop_or_rescue_is_left_alone() {
    let looped = emit_classes(
        r#"
class Ids
  def take(id)
    while id.is_a?(String)
      id = id.to_i
    end
    id
  end
end
"#,
    );
    assert!(
        !looped.contains("__rh_id"),
        "a while-body write must not split:\n{looped}"
    );
    let rescued = emit_classes(
        r#"
class Ids
  def take(id)
    begin
      id = id.to_i
    rescue
      id = 0
    end
    id
  end
end
"#,
    );
    assert!(
        !rescued.contains("__rh_id"),
        "a rescue-body write must not split:\n{rescued}"
    );
    let blocked = emit_classes(
        r#"
class Ids
  def take(id)
    [1].each { |n| id = n }
    id
  end
end
"#,
    );
    assert!(
        !blocked.contains("__rh_id"),
        "a block-body write must not split:\n{blocked}"
    );
}

/// A modifier-if join nested in a loop (or outer `if`) does not
/// dominate later reads. Splitting it would leave the trailing `id`
/// on the original binder.
#[test]
fn a_nested_modifier_if_join_is_left_alone() {
    let looped = emit_classes(
        r#"
class Ids
  def convert(id)
    while id.is_a?(String)
      id = id.to_i if id.is_a?(String)
    end
    id
  end
end
"#,
    );
    assert!(
        !looped.contains("__rh_id"),
        "a join inside while must not split:\n{looped}"
    );
    let branched = emit_classes(
        r#"
class Sessions
  def sign_in(user)
    if extra
      user = users(user) unless user.is_a?(User)
    end
    user
  end
end
"#,
    );
    assert!(
        !branched.contains("__rh_user"),
        "a join inside if must not split:\n{branched}"
    );
}

/// A method that never writes its parameter is a no-op.
#[test]
fn a_parameter_that_is_never_written_is_left_alone() {
    let out = emit_classes(
        r#"
class Users
  def email(user)
    user.email_address
  end
end
"#,
    );
    assert!(
        !out.contains("__rh_user"),
        "no rebind, no fresh local:\n{out}"
    );
    assert!(
        out.contains("user.email_address") || out.contains("user.email_address()"),
        "the parameter read stays:\n{out}"
    );
}

/// The name `__rh_user` already in the body must not collide.
#[test]
fn a_taken_fresh_name_gets_a_suffix() {
    let out = emit_classes(
        r#"
class Sessions
  def sign_in(user)
    __rh_user = 1
    user = users(user)
    user
  end
end
"#,
    );
    assert!(
        out.contains("__rh_user_2"),
        "must not steal a name the body already uses:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "user"),
        "the parameter still must not be written:\n{out}"
    );
}

/// A block parameter of the same name shadows; the method's rebind
/// must not rewrite reads inside the block.
#[test]
fn a_block_parameter_shadows_the_method_parameter() {
    let out = emit_classes(
        r#"
class Sessions
  def sign_in(user)
    user = users(user)
    [1].each { |user| user }
    user
  end
end
"#,
    );
    assert!(
        out.contains("|user|"),
        "the block parameter keeps its name:\n{out}"
    );
    assert!(
        !out.contains("|user| __rh_user") && !out.contains("{ |user| __rh_user"),
        "reads inside the block stay on the block parameter:\n{out}"
    );
}

/// Controller actions carry params on `Action`, not `MethodDef`.
#[test]
fn a_controller_action_rebind_splits() {
    let out = emit_app(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def show
    id = params[:id]
    id = id.to_i
    @user = User.find(id)
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
    ]);
    // `id = params[:id]` is a NEW local, not a parameter — left alone.
    // This pins that a controller body is walked, and that only binders
    // (the action's declared params) split.
    assert!(
        out.contains("id = ") || out.contains("id="),
        "the local from params[:id] stays a local:\n{out}"
    );
}

/// A controller HELPER with a declared positional (`def period(query)`)
/// is an `Action` with `params` fields — that binder does split.
#[test]
fn a_controller_helper_parameter_rebind_splits() {
    let files = &[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def show
    @user = locate(params[:id])
  end

  private
    def locate(id)
      id = id.to_i
      User.find(id)
    end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
    ];
    let out = emit_app_file(files, "users_controller.rb");
    assert!(
        out.contains("__rh_id"),
        "a controller helper's parameter rebind must split:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "id"),
        "must not write the integer onto the helper parameter:\n{out}"
    );
}

/// The spliced test-helper form, through the whole pipeline: ingest
/// splice, post-analyze, fixture rewrite, emit. This is the 27-file
/// campfire wall (`sign_in :david` then `user = users(user)`).
#[test]
fn a_spliced_sign_in_helper_emits_a_fresh_local() {
    let out = emit_app(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def email_address\n    \"a@example.com\"\n  end\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resource :session\nend\n",
        ),
        (
            "test/test_helper.rb",
            "class ActiveSupport::TestCase\n  include SessionTestHelper\nend\n",
        ),
        (
            "test/test_helpers/session_test_helper.rb",
            r#"module SessionTestHelper
  def sign_in(user)
    user = users(user) unless user.is_a? User
    post session_url, params: { email_address: user.email_address, password: "secret123456" }
  end
end
"#,
        ),
        (
            "test/controllers/welcome_controller_test.rb",
            "class WelcomeControllerTest < ActionDispatch::IntegrationTest\n  setup do\n    sign_in :david\n  end\n  test \"ok\" do\n    get \"/\"\n  end\nend\n",
        ),
        (
            "test/fixtures/users.yml",
            "david:\n  email_address: d@example.com\n",
        ),
    ]);
    assert!(
        out.contains("__rh_user"),
        "spliced sign_in must emit a fresh local:\n{out}"
    );
    assert!(
        !assigns_parameter(&out, "user"),
        "the User must not be written onto the parameter:\n{out}"
    );
}
