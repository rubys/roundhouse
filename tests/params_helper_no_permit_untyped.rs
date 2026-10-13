//! A private `*_params` helper with no recognized permit list used to
//! be pinned to `Hash[Symbol, untyped]` in its `.rbs` sidecar BY ITS
//! NAME ALONE (rubys/roundhouse#810). The suffix is a naming
//! convention, not a contract: `WidgetsController#check_kind_params` is
//! a guard filter whose body answers `nil` or the result of `head`, and
//! `spin build` refused the mismatch between that `bool`-shaped return
//! and the pinned `Hash[Symbol, untyped]` C struct pointer.
//!
//! With no recognized permit list, the helper is typed like any other
//! private helper: `untyped`, so the compiler infers the return from
//! the body. Campfire's `role_params` — a bare Hash literal returned
//! with no `permit` call, the case the fallback was written for — still
//! gets this `untyped` pin from roundhouse, but its EMITTED BODY is
//! untouched, so Spinel's own inference still lands on a Hash for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

/// The spinel tree for a small app: the base classes plus `files`.
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

fn sig<'a>(rbs: &'a str, method: &str) -> &'a str {
    let head = format!("def {method}: ");
    rbs.lines()
        .map(str::trim)
        .find(|l| l.starts_with(&head))
        .unwrap_or_else(|| panic!("no sig for {method}:\n{rbs}"))
}

/// The issue's minimal app: a `check_kind_params` guard `before_action`
/// whose body has no `permit` call at all.
const ROUTES: &str = "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show]\nend\n";
const SCHEMA: &str =
    "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n";
// A second `before_action` (`:touch_request`) that targets an ANCESTOR's
// method — needed so the inliner keeps `check_kind_params` as a method
// of its own instead of inlining it into `show` (see
// `own_filter_inlining_is_ordered`), which is how the sidecar's wrong
// pin on it becomes observable at all.
const APPLICATION_CONTROLLER: &str = r#"class ApplicationController < ActionController::API
  private

  def touch_request
    @touched = true
  end
end
"#;
const WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  KINDS = %w[small large].freeze

  before_action :check_kind_params, only: :show
  before_action :touch_request

  def index
    render json: { ok: true }
  end

  def show
    render json: { kind: @kind }
  end

  private

  def check_kind_params
    @kind = params.require(:kind)
    head(:not_found) unless KINDS.include?(@kind)
  end
end
"#;

/// The sidecar for a `check_kind_params`-style guard filter — a
/// `*_params`-suffixed helper with no recognized permit list — has no
/// `Hash[Symbol, untyped]` pin. Fails before the fix (the old fallback
/// pinned it to `Hash[Symbol, untyped]`), passes after (`untyped`).
#[test]
fn a_no_permit_guard_filter_sidecar_is_not_pinned_to_hash() {
    let files = spinel(&[
        ("config/routes.rb", ROUTES),
        ("db/schema.rb", SCHEMA),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
    ]);
    let rbs = file(&files, "app/controllers/widgets_controller.rbs");
    let check_kind = sig(rbs, "check_kind_params");
    assert!(
        !check_kind.contains("Hash[Symbol, untyped]"),
        "check_kind_params must not be pinned to a Hash return:\n{check_kind}"
    );
    assert_eq!(check_kind, "def check_kind_params: () -> untyped", "{check_kind}");
}

/// Campfire's `role_params` builds a bare Hash literal and calls no
/// `permit`, so it is the SAME no-recognized-permit-list case as
/// `check_kind_params` above: roundhouse pins it `untyped` too now
/// (same fallback, same reasoning). What must NOT regress is the
/// emitted BODY — unchanged, still a Hash literal — so Spinel's own
/// type inference lands on a Hash for it, same as before the fix.
const ROLE_ROUTES: &str = "Rails.application.routes.draw do\n  resources :users, only: [:update]\nend\n";
const ROLE_SCHEMA: &str =
    "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n";
const USERS_CONTROLLER: &str = r#"class UsersController < ApplicationController
  def update
    @user = User.find(params[:id])
    @user.update(role_params)
  end

  private

  def role_params
    { role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member" }
  end
end
"#;

#[test]
fn campfire_style_role_params_still_builds_a_hash_in_its_body() {
    let files = spinel(&[
        ("config/routes.rb", ROLE_ROUTES),
        ("db/schema.rb", ROLE_SCHEMA),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/controllers/users_controller.rb", USERS_CONTROLLER),
    ]);
    let rbs = file(&files, "app/controllers/users_controller.rbs");
    let role_params = sig(rbs, "role_params");
    // No longer pinned to a Hash by the fallback — it follows the same
    // rule as any other no-recognized-permit-list helper.
    assert_eq!(role_params, "def role_params: () -> untyped", "{role_params}");

    let rb = file(&files, "app/controllers/users_controller.rb");
    assert!(
        rb.contains("role:") && rb.contains('{') && rb.contains('}'),
        "role_params must still build a Hash literal in its body:\n{rb}"
    );
}
