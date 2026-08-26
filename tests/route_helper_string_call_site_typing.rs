//! A route segment's type comes from the CALL SITE when the name-based
//! rule would guess wrong.
//!
//! `param_ty` reads the segment's NAME and answers Int for `id` /
//! `<x>_id`. That is a claim about the value, and a name cannot support
//! it: campfire writes `resources :qr_code` — no `QrCode` model behind
//! it anywhere — and calls `qr_code_path(Base64.urlsafe_encode64(url))`.
//! The generated signature said `(Integer id)` against a String, and on
//! a strict target that is not a warning: spinel refuses the build with
//! "a seed is trusted, so the emitted code would reinterpret the value
//! rather than convert it."
//!
//! Answering it by NAME the other way — no model behind the route ⇒
//! String — was tried and is wrong in both directions: a route's name
//! flattens what a model's name nests (`Rooms::OpensController` ⇒
//! `rooms_open`, model `Rooms::Open`), and an `id` routinely belongs to
//! a model the route never names (campfire's `account_bot_path` carries
//! a User id). Both misses retype a segment whose call sites pass
//! `record.id`, breaking the emit in the direction the change was meant
//! to fix. Measured: 20 signatures moved, 19 of them wrong.
//!
//! So the evidence is the call site, read off the analyzer's type. A
//! helper nobody calls keeps the name-based default — which is what the
//! second half of this test pins.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :articles\n  \
    resources :qr_code, only: :show\nend\n";

// One helper module, two calls: the qr code's `:id` is a Base64 blob,
// the article's is a primary key.
const HELPER: &str = r#"
module LinkHelper
  def zoom_qr_code(url)
    token = Base64.urlsafe_encode64(url)
    qr_code_path(token)
  end

  def article_link(article)
    article_path(article.id)
  end
end
"#;

fn helper_params(name: &str) -> Vec<roundhouse::ty::Param> {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/helpers/link_helper.rb", HELPER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    // The survey reads `Expr::ty`, which only the analyzer populates —
    // lowering the routes off a raw ingest would silently find nothing
    // and the test would pass for the wrong reason.
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| panic!("helper {name} not generated"))
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("helper signature is not Ty::Fn") };
    params
}

#[test]
fn string_call_site_retypes_an_id_segment() {
    let params = helper_params("qr_code_path");
    assert_eq!(params[0].name.as_str(), "id");
    assert!(
        matches!(params[0].ty, Ty::Str),
        "an `id` segment a call site fills with a String must type Str, got {:?}",
        params[0].ty
    );
}

#[test]
fn an_id_call_site_keeps_the_integer_default() {
    let params = helper_params("article_path");
    assert_eq!(params[0].name.as_str(), "id");
    assert!(
        matches!(params[0].ty, Ty::Int),
        "a model-backed `id` segment must stay Int, got {:?}",
        params[0].ty
    );
}

/// The evidence chain the campfire spinel lane actually needed, end to
/// end: a signed id minted in a model CONCERN reaches a route helper's
/// signature as a String.
///
/// Three things have to hold at once, and each was broken:
///   * `signed_id` has to be AR catalog surface, so it answers `Str`.
///     `lower::signed_id` rewrites the call to
///     `ActiveRecord::SignedId.generate` — whose runtime RBS already
///     says `-> String` — but that pass runs POST-analyze, long after
///     the method wrapping it was typed.
///   * a module a MODEL includes has to see the AR instance surface at
///     all. A concern's body is typed as a library class with `self_ty`
///     set to the MODULE, so a bare self-send inside one dispatches
///     against the module and nothing else.
///   * only then does the existing call-site rule above have a `Ty::Str`
///     to read.
///
/// campfire's shape exactly: `User::Transferable#transfer_id` is a
/// one-line `signed_id(purpose: :transfer)`, and the view writes
/// `session_transfer_url(user.transfer_id)`. Typed `untyped`, the
/// segment kept its name-based Integer default and spinel refused the
/// build — with the emitted code correct and the CRuby lane green.
#[test]
fn a_signed_id_from_a_concern_retypes_the_segment() {
    const ROUTES: &str = "Rails.application.routes.draw do\n  \
        resources :transfers, only: :show\nend\n";
    let tree = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("config/routes.rb", ROUTES),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Transferable\nend\n",
        ),
        (
            "app/models/user/transferable.rb",
            r#"module User::Transferable
  extend ActiveSupport::Concern

  def transfer_id
    signed_id(purpose: :transfer)
  end
end
"#,
        ),
        (
            "app/helpers/transfer_helper.rb",
            r#"
module TransferHelper
  # The receiver has to be TYPED for the call-site rule to have
  # anything to read. campfire's real site is a view local whose
  # partial declares it; an untyped helper parameter reproduces
  # nothing, and an earlier draft of this fixture used one and failed
  # for that reason rather than the one under test.
  def transfer_link
    user = User.new
    transfer_path(user.transfer_id)
  end
end
"#,
        ),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == "transfer_path")
        .expect("transfer_path not generated")
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("not Ty::Fn") };
    assert_eq!(params[0].name.as_str(), "id");
    assert!(
        matches!(params[0].ty, Ty::Str),
        "a segment filled by a concern's signed id must type Str, got {:?}",
        params[0].ty
    );
}

/// A `direct` helper's body is a CALL SITE too — and the only one some
/// routes have.
///
/// campfire declares
///
///     direct :fresh_user_avatar do |user, options|
///       route_for :user_avatar, user.avatar_token, v: …
///     end
///
/// and `user_avatar_path` is reached from nowhere else. `avatar_token`
/// is a `signed_id`, so that segment is a String; the controller agrees
/// (`User.from_avatar_token(params[:user_id])`). Two things had to
/// change for the rule above to see it: these bodies live in
/// `config/routes.rb` and were never analyzed, so every node carried
/// `ty: None`; and `route_for` names its target with a SYMBOL rather
/// than calling the helper, so the walk did not recognise it as a call
/// site at all.
///
/// The block parameter is seeded from the helper's own call sites,
/// which is the same evidence rule — and the same reason — as the
/// segment typing it feeds.
#[test]
fn a_direct_helpers_route_for_is_a_call_site() {
    let params = direct_helper_params(
        "module AvatarHelper\n  \
           def avatar_link\n    \
             user = User.new\n    \
             fresh_user_avatar_path(user)\n  \
           end\nend\n",
    );
    assert_eq!(params[0].name.as_str(), "user_id");
    assert!(
        matches!(params[0].ty, Ty::Str),
        "a segment a `direct` block fills with a signed id must type Str, got {:?}",
        params[0].ty
    );
}

/// The seed's other half: an UNTYPED call site is absence of evidence,
/// not evidence of untyped. Unioning it in poisons the parameter — an
/// `Untyped` arm ABSORBS dispatch, so `User | Untyped` answers
/// `Untyped` for `avatar_token` and the segment silently keeps its
/// name-based default.
///
/// campfire has exactly this mix — five typed callers and two untyped
/// — and before the filter the untyped ones won. The gradual shape is
/// its `_direct.html.erb`: `members = membership.room.users
/// .without(…).presence || [ membership.user ]`, then
/// `fresh_user_avatar_path(members.first)`. A `.presence ||` over a
/// collection is what types Untyped, and this fixture mirrors it —
/// an untyped METHOD PARAMETER does not reproduce it, because that
/// types `Var`, which the `is_open` guard already skips. An earlier
/// draft used one and passed with the filter ablated.
#[test]
fn an_untyped_call_site_does_not_poison_the_direct_seed() {
    let params = direct_helper_params(
        "module AvatarHelper\n  \
           def typed_link\n    \
             user = User.new\n    \
             fresh_user_avatar_path(user)\n  \
           end\n  \
           def group_link(room)\n    \
             members = room.users.presence || [ User.new ]\n    \
             fresh_user_avatar_path(members.first)\n  \
           end\nend\n",
    );
    assert!(
        matches!(params[0].ty, Ty::Str),
        "an untyped caller must not outvote the typed ones, got {:?}",
        params[0].ty
    );
}

/// `user_avatar_path`'s parameters, with `helper` as the app's only
/// helper module. The route is `resources :users do resource :avatar
/// end`, so the segment is named `user_id` and the name-based rule
/// answers Int for it — which is what the direct body has to overrule.
fn direct_helper_params(helper: &str) -> Vec<roundhouse::ty::Param> {
    let tree = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :users do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  \
               resources :users do\n    resource :avatar, only: :show\n  end\n  \
               direct :fresh_user_avatar do |user, options|\n    \
                 route_for :user_avatar, user.avatar_token\n  \
               end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Avatar\nend\n",
        ),
        (
            "app/models/user/avatar.rb",
            r#"module User::Avatar
  extend ActiveSupport::Concern

  def avatar_token
    signed_id(purpose: :avatar)
  end
end
"#,
        ),
        ("app/helpers/avatar_helper.rb", helper),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == "user_avatar_path")
        .expect("user_avatar_path not generated")
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("not Ty::Fn") };
    params
}
