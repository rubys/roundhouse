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
