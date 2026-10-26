//! Route-helper naming for custom member/collection routes, the
//! symbol-form verb shortcut, and `resources ..., as:`.
//!
//! Rails' auto-naming is scope-sensitive in a way that a single
//! `<parent>_<child>` rule cannot express:
//!
//! ```text
//! resources :comments do
//!   member do post "disown" end      → disown_comment_path      (action + SINGULAR)
//!   collection do get "requested" end → requested_comments_path  (action + PLURAL)
//!   get "suggest"                     → comment_suggest_path     (SINGULAR + action)
//! end
//! ```
//!
//! Only the third is `<parent>_<child>`, and it is the one shape the
//! flattener used to apply to all three — so every custom member route
//! in lobsters (the disown / reply / doff / update_in_place families)
//! got a helper name no call site referred to, while the call sites
//! stayed bare and unresolved.

use roundhouse::App;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::flatten_routes;

fn helpers(routes_rb: &str) -> Vec<(String, String)> {
    let table = ingest_routes(routes_rb.as_bytes(), "config/routes.rb").expect("ingest routes");
    let mut app = App::default();
    app.routes = table;
    flatten_routes(&app)
        .into_iter()
        .filter(|r| r.named)
        .map(|r| (r.as_name.clone(), r.path.clone()))
        .collect()
}

fn name_for(routes_rb: &str, path: &str, expect: &str) {
    let all = helpers(routes_rb);
    let found: Vec<&(String, String)> = all.iter().filter(|(_, p)| p == path).collect();
    assert!(
        found.iter().any(|(n, _)| n == expect),
        "expected a `{expect}` helper for {path}; got {found:?}\nall: {all:?}"
    );
}

const COMMENTS: &str = r#"Rails.application.routes.draw do
  resources :comments, except: [:new, :destroy] do
    member do
      get "reply"
      post "disown"
    end
    collection do
      get "requested"
    end
    get "suggest"
  end
end
"#;

#[test]
fn member_routes_are_named_action_first_and_singular() {
    // Rails: `disown_comment_path`, not `comment_disown_path`.
    name_for(COMMENTS, "/comments/:id/disown", "disown_comment");
    name_for(COMMENTS, "/comments/:id/reply", "reply_comment");
}

#[test]
fn collection_routes_are_named_action_first_and_plural() {
    name_for(COMMENTS, "/comments/requested", "requested_comments");
}

#[test]
fn bare_nested_routes_keep_the_parent_first_order() {
    // The one shape that really is `<singular-parent>_<child>`: a verb
    // declared directly in the resources block is a NESTED route under
    // `/:comment_id`, and Rails names it parent-first.
    name_for(COMMENTS, "/comments/:comment_id/suggest", "comment_suggest");
}

#[test]
fn symbol_form_member_routes_are_not_dropped() {
    // lobsters' hats routes use `get :doff` exclusively. Accepting only
    // the String positional silently dropped all four routes AND their
    // helpers — no diagnostic, just missing surface.
    let src = r#"Rails.application.routes.draw do
  resources :hats, only: %i[index edit] do
    member do
      get :doff
      post :update_in_place
    end
  end
end
"#;
    name_for(src, "/hats/:id/doff", "doff_hat");
    name_for(src, "/hats/:id/update_in_place", "update_in_place_hat");
}

#[test]
fn symbol_and_string_forms_agree() {
    let sym = helpers(
        "Rails.application.routes.draw do\n  resources :hats do\n    member do\n      get :doff\n    end\n  end\nend\n",
    );
    let string = helpers(
        "Rails.application.routes.draw do\n  resources :hats do\n    member do\n      get \"doff\"\n    end\n  end\nend\n",
    );
    assert_eq!(sym, string, "`get :doff` must route exactly like `get \"doff\"`");
}

const NAMESPACED_AS: &str = r#"Rails.application.routes.draw do
  resources :mod_mails, only: [:index, :show]
  namespace :mod do
    resources :mails, except: [:destroy], as: "mod_mails"
  end
end
"#;

#[test]
fn resources_as_renames_the_helper_but_not_the_path() {
    // Rails: `/mod/mails` served by `mod_mod_mails_path` — the namespace
    // prefix still applies on top of the `as:` name. Dropping `as:`
    // named it `mod_mails_path`, which collided with the top-level
    // `resources :mod_mails` below and missed every call site.
    name_for(NAMESPACED_AS, "/mod/mails", "mod_mod_mails");
    name_for(NAMESPACED_AS, "/mod/mails/:id", "mod_mod_mail");
    name_for(NAMESPACED_AS, "/mod/mails/:id/edit", "edit_mod_mod_mail");
    name_for(NAMESPACED_AS, "/mod/mails/new", "new_mod_mod_mail");
}

#[test]
fn the_unrenamed_sibling_keeps_its_own_helpers() {
    // The top-level `resources :mod_mails` must still own
    // `mod_mails_path` / `mod_mail_path` — proof the rename didn't
    // simply move the collision.
    name_for(NAMESPACED_AS, "/mod_mails", "mod_mails");
    name_for(NAMESPACED_AS, "/mod_mails/:id", "mod_mail");
}

// A member/collection route puts its action in front of EVERYTHING the
// enclosing scopes contribute: the namespace or `scope as:` prefix and
// the outer resources' singulars. Expected names are Rails 8.1.4's
// (`bin/rails routes` on the same route sets).

fn no_helper(routes_rb: &str, name: &str) {
    let all = helpers(routes_rb);
    assert!(
        all.iter().all(|(n, _)| n != name),
        "no `{name}` helper expected (Rails has none); got {all:?}"
    );
}

#[test]
fn namespaced_member_and_collection_routes_put_the_action_first() {
    // #728's reduction: the app's own `batch_admin_widgets_path` call
    // did not resolve because the helper came out `admin_batch_widgets`.
    let concern = r#"Rails.application.routes.draw do
  concern :batch do
    collection do
      post :batch
    end
  end

  namespace :admin do
    resources :widgets, only: [:index], concerns: :batch
  end
end
"#;
    name_for(concern, "/admin/widgets/batch", "batch_admin_widgets");
    no_helper(concern, "admin_batch_widgets");

    let blocks = r#"Rails.application.routes.draw do
  namespace :admin do
    resources :widgets do
      member { post :approve }
      collection { get :search }
    end
  end
end
"#;
    name_for(blocks, "/admin/widgets/:id/approve", "approve_admin_widget");
    name_for(blocks, "/admin/widgets/search", "search_admin_widgets");
    no_helper(blocks, "admin_approve_widget");
    no_helper(blocks, "admin_search_widgets");
}

#[test]
fn an_explicit_as_on_a_namespaced_member_route_goes_first_too() {
    let src = r#"Rails.application.routes.draw do
  namespace :admin do
    resources :rooms do
      get :preview, on: :member, as: :peek
      get :recent, on: :collection, as: :fresh
      get :latest, on: :collection
    end
  end
end
"#;
    name_for(src, "/admin/rooms/:id/preview", "peek_admin_room");
    name_for(src, "/admin/rooms/recent", "fresh_admin_rooms");
    name_for(src, "/admin/rooms/latest", "latest_admin_rooms");
    no_helper(src, "admin_peek_room");
    no_helper(src, "admin_fresh_rooms");
}

#[test]
fn a_scope_as_prefix_goes_after_the_action() {
    let src = r#"Rails.application.routes.draw do
  scope as: :v1 do
    resources :widgets do
      collection { get :search }
    end
  end
end
"#;
    name_for(src, "/widgets/search", "search_v1_widgets");
    no_helper(src, "v1_search_widgets");
}

#[test]
fn a_nested_resource_member_route_keeps_its_outer_parents() {
    // The outer resource's singular sits between the action and the
    // innermost resource; it was dropped (`pin_note`, `fresh_rooms`).
    let src = r#"Rails.application.routes.draw do
  resources :users do
    resources :notes do
      member { post :pin }
      collection { get :archived }
    end
    resources :rooms do
      get :preview, on: :member, as: :peek
      get :recent, on: :collection, as: :fresh
    end
  end
  resource :account do
    resources :bots do
      member { post :reset }
    end
  end
  namespace :admin do
    resources :users do
      resources :notes do
        member { post :pin }
      end
    end
  end
end
"#;
    name_for(src, "/users/:user_id/notes/:id/pin", "pin_user_note");
    name_for(src, "/users/:user_id/notes/archived", "archived_user_notes");
    name_for(src, "/users/:user_id/rooms/:id/preview", "peek_user_room");
    name_for(src, "/users/:user_id/rooms/recent", "fresh_user_rooms");
    name_for(src, "/account/bots/:id/reset", "reset_account_bot");
    name_for(src, "/admin/users/:user_id/notes/:id/pin", "pin_admin_user_note");
    for name in ["pin_note", "archived_notes", "peek_room", "fresh_rooms", "reset_bot"] {
        no_helper(src, name);
    }
    no_helper(src, "admin_pin_note");
}

#[test]
fn a_bare_verb_in_a_namespaced_resource_stays_parent_first() {
    // The action-first rule is for member/collection routes only: a
    // verb declared directly in the block is a NESTED route.
    let src = r#"Rails.application.routes.draw do
  namespace :admin do
    resources :widgets do
      get :suggest
    end
  end
end
"#;
    name_for(src, "/admin/widgets/:widget_id/suggest", "admin_widget_suggest");
}
