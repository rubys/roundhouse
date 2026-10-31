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
//! A singular `resource` has no id to nest the third under, so Rails
//! takes it as a member route there: `resource :account do get "welcome"
//! end` is `welcome_account_path`.
//!
//! Only the third is `<parent>_<child>`, and it is the one shape the
//! flattener used to apply to all three — so every custom member route
//! in lobsters (the disown / reply / doff / update_in_place families)
//! got a helper name no call site referred to, while the call sites
//! stayed bare and unresolved.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::App;
use roundhouse::dialect::HttpMethod;
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

/// A namespaced route Rails cannot name from its path (an optional or
/// dynamic segment, no `as:`) is named for the namespace itself, the first
/// such route taking it — Rails 8.1 names lobsters'
/// `namespace :mod { get "notes(/:period)", to: redirect("/mod/") }` `mod`,
/// and the moderator subnav links `mod_path`.
#[test]
fn an_unnameable_namespaced_route_takes_the_namespace_name() {
    let routes = r#"Rails.application.routes.draw do
  namespace :mod do
    get "/" => "activities#index", :as => "mod_activity"
    get "notes(/:period)", to: redirect("/mod/")
    get "flagged/:period" => "flagged#index"
  end
end
"#;
    let all = helpers(routes);
    let mods: Vec<&(String, String)> = all.iter().filter(|(n, _)| n == "mod").collect();
    assert_eq!(mods.first().map(|(_, p)| p.as_str()), Some("/mod/notes/:period"), "{all:?}");
    name_for(routes, "/mod", "mod_mod_activity");
}

fn unnamed(routes_rb: &str, path: &str) {
    let all = helpers(routes_rb);
    assert!(
        all.iter().all(|(_, p)| p != path),
        "no helper expected for {path} (Rails names none); got {all:?}"
    );
}

// A bare verb in a SINGULAR resource is a member route — there is no id
// to nest under — so it reads action-first like one, with every scope
// prefix and outer resource in between. It was named parent-first
// (`profile_preview`), so a call to Rails' `preview_profile_path`
// raised NoMethodError. Rails 8.1.4 (`bin/rails routes`) for each:
const SINGULAR: &str = r#"Rails.application.routes.draw do
  resource :profile do
    get :preview
    post :refresh
    get :download, as: :dl
    get "contact-info"
    get :theme, to: "themes#show"
    get "photo/:size", to: "profiles#photo"
    resources :links do
      get :clicks
      resource :label do
        get :print
      end
    end
  end
  namespace :admin do
    resource :console do
      get :logs
    end
  end
  scope module: :api do
    resource :clock do
      get :time
    end
  end
  scope path: "/v1" do
    resource :mailbox do
      get :unread
    end
  end
  scope "/old", as: :old do
    resource :studio do
      get :overview
    end
  end
  resource :garage, as: :shed do
    get :tools
  end
  resources :teams do
    resource :calendar do
      get :agenda
      resources :days do
        resource :note do
          get :draft
        end
      end
    end
  end
end
"#;

#[test]
fn a_bare_verb_in_a_singular_resource_is_named_action_first() {
    name_for(SINGULAR, "/profile/preview", "preview_profile");
    name_for(SINGULAR, "/profile/refresh", "refresh_profile");
    name_for(SINGULAR, "/profile/download", "dl_profile");
    name_for(SINGULAR, "/profile/contact-info", "contact_info_profile");
    name_for(SINGULAR, "/profile/theme", "theme_profile");
    for name in ["profile_preview", "profile_refresh", "profile_dl", "profile_contact_info"] {
        no_helper(SINGULAR, name);
    }
}

#[test]
fn a_singular_resource_bare_verb_puts_every_scope_prefix_after_the_action() {
    // The namespace and `scope as:` names follow the action; a `scope
    // path:` or `module:` contributes no name at all, and a resource's
    // `as:` replaces its own name.
    name_for(SINGULAR, "/admin/console/logs", "logs_admin_console");
    name_for(SINGULAR, "/clock/time", "time_clock");
    name_for(SINGULAR, "/v1/mailbox/unread", "unread_mailbox");
    name_for(SINGULAR, "/old/studio/overview", "overview_old_studio");
    name_for(SINGULAR, "/garage/tools", "tools_shed");
    for name in [
        "admin_console_logs",
        "clock_time",
        "v1_mailbox_unread",
        "old_studio_overview",
        "garage_tools",
    ] {
        no_helper(SINGULAR, name);
    }
}

#[test]
fn a_singular_resource_bare_verb_keeps_its_outer_resources() {
    name_for(SINGULAR, "/teams/:team_id/calendar/agenda", "agenda_team_calendar");
    name_for(SINGULAR, "/profile/links/:link_id/label/print", "print_profile_link_label");
    name_for(
        SINGULAR,
        "/teams/:team_id/calendar/days/:day_id/note/draft",
        "draft_team_calendar_day_note",
    );
    for name in ["calendar_agenda", "label_print", "note_draft"] {
        no_helper(SINGULAR, name);
    }
}

#[test]
fn a_singular_resource_bare_verb_with_a_segment_gets_no_name() {
    unnamed(SINGULAR, "/profile/photo/:size");
}

// A bare verb in a PLURAL resource is nested under the parent's id and
// reads parent-first — but parent means EVERY enclosing resource, not
// just the innermost one. Only that one was kept (`book_cover`).
const DEEP: &str = r#"Rails.application.routes.draw do
  resources :authors do
    resources :books do
      get :cover
      resources :chapters do
        get :history
        get :history, to: "chapters#history2"
      end
    end
  end
  namespace :ns do
    resources :authors do
      resources :books do
        get :cover
      end
    end
  end
  resource :profile do
    resources :links do
      get :clicks
      get :visits, as: :v
    end
  end
  namespace :admin do
    resource :console do
      resources :widgets do
        get :info
      end
    end
  end
  resource :garage, as: :shed do
    resources :slots, as: :bays do
      get :sweep
    end
  end
  resources :people, as: :folks, path: "humans" do
    get :avatar
  end
end
"#;

#[test]
fn a_nested_bare_verb_keeps_every_outer_resource() {
    name_for(DEEP, "/authors/:author_id/books/:book_id/cover", "author_book_cover");
    name_for(
        DEEP,
        "/authors/:author_id/books/:book_id/chapters/:chapter_id/history",
        "author_book_chapter_history",
    );
    name_for(DEEP, "/ns/authors/:author_id/books/:book_id/cover", "ns_author_book_cover");
    name_for(DEEP, "/profile/links/:link_id/clicks", "profile_link_clicks");
    name_for(DEEP, "/profile/links/:link_id/visits", "profile_link_v");
    name_for(DEEP, "/admin/console/widgets/:widget_id/info", "admin_console_widget_info");
    for name in ["book_cover", "chapter_history", "ns_book_cover", "link_clicks", "link_v", "admin_widget_info"] {
        no_helper(DEEP, name);
    }
}

#[test]
fn a_nested_bare_verb_reads_the_resources_as_names() {
    name_for(DEEP, "/garage/slots/:bay_id/sweep", "shed_bay_sweep");
    name_for(DEEP, "/humans/:folk_id/avatar", "folk_avatar");
    no_helper(DEEP, "bay_sweep");
}

#[test]
fn a_repeated_verb_shares_the_first_routes_helper() {
    // Rails drops an automatic name an earlier route already took, so the
    // second `history` is unnamed rather than a second claim on the name.
    let table = ingest_routes(DEEP.as_bytes(), "config/routes.rb").expect("ingest routes");
    let mut app = App::default();
    app.routes = table;
    let history: Vec<(String, bool)> = flatten_routes(&app)
        .into_iter()
        .filter(|r| r.path.ends_with("/history"))
        .map(|r| (r.action.as_str().to_string(), r.named))
        .collect();
    assert_eq!(history, [("history".to_string(), true), ("history2".to_string(), false)]);

    let verbs = "Rails.application.routes.draw do\n  resource :profile do\n    get :preview\n    post :preview\n  end\nend\n";
    let preview: Vec<(String, String)> =
        helpers(verbs).into_iter().filter(|(_, p)| p == "/profile/preview").collect();
    assert_eq!(preview, [("preview_profile".to_string(), "/profile/preview".to_string())]);
}

#[test]
fn a_bare_verb_names_like_its_member_or_collection_spelling() {
    // In a singular resource the bare verb IS a member route; in a plural
    // one it is nested, and `on:`/the blocks still read action-first.
    let src = r#"Rails.application.routes.draw do
  resource :account do
    get :welcome
    get :peek, on: :member
    member { get :avatar }
    get :later, on: :collection
    collection { get :history }
    get :archive, on: :member, as: :past
  end
  resources :teams do
    get :roster
    get :roster_all, on: :member
    collection { get :export }
  end
end
"#;
    name_for(src, "/account/welcome", "welcome_account");
    name_for(src, "/account/peek", "peek_account");
    name_for(src, "/account/avatar", "avatar_account");
    name_for(src, "/account/later", "later_account");
    name_for(src, "/account/history", "history_account");
    name_for(src, "/account/archive", "past_account");
    name_for(src, "/teams/:team_id/roster", "team_roster");
    name_for(src, "/teams/:id/roster_all", "roster_all_team");
    name_for(src, "/teams/export", "export_teams");
}

#[test]
fn a_canonical_action_is_named_only_outside_a_member_or_collection() {
    // `show`/`new`/… name nothing of their own on a resource's member or
    // collection (they ARE the resource's own actions there), but nested
    // under a plural resource they are as ordinary as any other word.
    let src = r#"Rails.application.routes.draw do
  resources :teams do
    get :show
    get :new
  end
end
"#;
    name_for(src, "/teams/:team_id/show", "team_show");
    name_for(src, "/teams/:team_id/new", "team_new");
}

// A canonical action written as a SYMBOL on a member or collection is
// the resource's own route: Rails 8.1.4 gives it the scope's path, with
// no `/<action>`, and the resource's own name — which, registered
// before the resource's defaults, leaves the matching default unnamed.
// Written as a string it is a path like any other.
const CANONICAL: &str = r#"Rails.application.routes.draw do
  namespace :admin do
    resources :photos do
      collection do
        get :index
        get :summary
      end
    end
  end
  scope ":org" do
    resource :banner, only: [:show, :edit, :update] do
      member { post :update }
    end
    namespace :prefs do
      resources :alerts, only: [:index] do
        patch :update, on: :collection
      end
    end
  end
  resources :notes do
    collection { get "index" }
    member { get "show" }
    get :new, on: :collection
  end
  resource :label do
    get :new
  end
end
"#;

fn flat(routes_rb: &str) -> Vec<roundhouse::lower::routes::FlatRoute> {
    let mut app = App::default();
    app.routes = ingest_routes(routes_rb.as_bytes(), "config/routes.rb").expect("ingest routes");
    flatten_routes(&app)
}

#[test]
fn a_canonical_action_symbol_takes_its_resources_own_path() {
    name_for(CANONICAL, "/admin/photos", "admin_photos");
    name_for(CANONICAL, "/admin/photos/summary", "summary_admin_photos");
    name_for(CANONICAL, "/:org/banner", "banner");
    name_for(CANONICAL, "/:org/prefs/alerts", "prefs_alerts");
    let routes = flat(CANONICAL);
    let at = |verb: HttpMethod, path: &str| {
        routes
            .iter()
            .filter(|r| r.method == verb && r.path == path)
            .map(|r| format!("{}#{}", r.controller.0.as_str(), r.action.as_str()))
            .collect::<Vec<_>>()
    };
    assert_eq!(at(HttpMethod::Post, "/:org/banner"), ["BannersController#update"]);
    assert_eq!(at(HttpMethod::Patch, "/:org/prefs/alerts")[0], "Prefs::AlertsController#update");
    assert_eq!(at(HttpMethod::Get, "/notes")[0], "NotesController#new");
    assert_eq!(at(HttpMethod::Get, "/label")[0], "LabelsController#new");
    for path in ["/admin/photos/index", "/:org/banner/update", "/:org/prefs/alerts/update"] {
        assert!(
            routes.iter().all(|r| r.path != path),
            "{path} came out with the action appended: {routes:?}"
        );
    }
}

#[test]
fn a_canonical_action_symbol_leaves_the_matching_default_unnamed() {
    // The block's route is registered first and takes the name; Rails
    // drops the same automatic name from the default `index`/`show`.
    let named: Vec<(String, String, String)> = flat(CANONICAL)
        .into_iter()
        .filter(|r| r.named && ["admin_photos", "banner"].contains(&r.as_name.as_str()))
        .map(|r| (r.as_name, format!("{:?}", r.method), r.action.as_str().to_string()))
        .collect();
    assert_eq!(
        named,
        [
            ("admin_photos".to_string(), "Get".to_string(), "index".to_string()),
            ("banner".to_string(), "Post".to_string(), "update".to_string()),
        ]
    );
}

#[test]
fn a_canonical_action_string_is_a_path_but_still_names_nothing() {
    // `get "index"` keeps its `/index` — and is still the canonical
    // action for naming, so it takes the resource's own name, as in Rails.
    name_for(CANONICAL, "/notes/index", "notes");
    name_for(CANONICAL, "/notes/:id/show", "note");
    no_helper(CANONICAL, "index_notes");
    no_helper(CANONICAL, "show_note");
}

#[test]
fn a_slash_written_canonical_action_is_a_new_action() {
    // Rails compares the action exactly as written: `get "/show"` is no
    // canonical `show`, so it is named for itself and the resource keeps
    // its own `memo_path`.
    let src = r#"Rails.application.routes.draw do
  resources :memos do
    get "/show", on: :member, to: "memos#show"
  end
end
"#;
    name_for(src, "/memos/:id/show", "show_memo");
    name_for(src, "/memos/:id", "memo");
}

#[test]
fn a_canonical_action_on_a_root_mounted_resource_is_the_root_path() {
    let src = r#"Rails.application.routes.draw do
  resources :entries, path: "/" do
    collection { get :index }
  end
  scope "/home" do
    resources :feeds, path: "/" do
      collection { get :index }
    end
  end
end
"#;
    name_for(src, "/", "entries");
    name_for(src, "/home", "feeds");
}

#[test]
fn a_canonical_action_nested_in_a_plural_resource_is_an_ordinary_path() {
    // Nested under the parent's id is no member or collection: the
    // action is a segment and a name part like any other word.
    let src = "Rails.application.routes.draw do\n  resources :crates do\n    get :index\n    get :show\n  end\nend\n";
    name_for(src, "/crates/:crate_id/index", "crate_index");
    name_for(src, "/crates/:crate_id/show", "crate_show");
    name_for(src, "/crates", "crates");
}

#[test]
fn canonical_action_helpers_build_the_resources_own_url() {
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  resources :articles do
    collection { get :index }
    resources :comments, only: [:create, :destroy]
  end
  scope ":org" do
    resource :banner, controller: "articles", only: [:show] do
      member { post :update, to: "articles#update" }
    end
  end
  resources :drafts, controller: "articles", only: [:index] do
    patch :update, on: :collection, to: "articles#update"
  end
end
"#,
        )
        .run_ruby(
            r##"
[
  [RouteHelpers.articles_path, "/articles", "GET", :index],
  [RouteHelpers.banner_path("acme"), "/acme/banner", "POST", :update],
  [RouteHelpers.drafts_path, "/drafts", "PATCH", :update],
].each do |path, want, verb, action|
  raise "helper built #{path}, want #{want}" unless path == want
  matched = ActionDispatch::Router.match(verb, path, RouteTable.table)
  raise "no route for #{verb} #{path}" if matched.nil?
  raise "#{verb} #{path}: #{matched.controller}##{matched.action}" unless
    matched.controller == :articles && matched.action == action
end
show = ActionDispatch::Router.match("GET", "/acme/banner", RouteTable.table)
raise "GET /acme/banner: #{show&.action}" unless show && show.action == :show
puts "PASS canonical action helpers"
"##,
        )
        .assert_passes();
}

#[test]
fn a_hyphenated_or_slashed_name_part_becomes_underscores() {
    let src = r#"Rails.application.routes.draw do
  resources :users do
    get :ping, on: :member, as: "pong-check"
    get "fun-facts"
  end
  get "/help-center", to: "pages#help"
  get "/docs/api", to: "pages#api"
end
"#;
    name_for(src, "/users/:id/ping", "pong_check_user");
    name_for(src, "/users/:user_id/fun-facts", "user_fun_facts");
    name_for(src, "/help-center", "help_center");
    name_for(src, "/docs/api", "docs_api");
}

#[test]
fn a_dynamic_route_in_a_resource_is_named_only_by_an_explicit_as() {
    let src = r#"Rails.application.routes.draw do
  resources :teams do
    get ":handle/info", to: "teams#info"
    get "info/:handle", to: "teams#info2", as: :handle_info
  end
  resources :rooms do
    nested do
      scope path: ":bot_key" do
        get :hook, to: "rooms#hook"
        get "raw/:id", to: "rooms#raw"
      end
    end
  end
end
"#;
    unnamed(src, "/teams/:team_id/:handle/info");
    name_for(src, "/teams/:team_id/info/:handle", "team_handle_info");
    name_for(src, "/rooms/:room_id/:bot_key/hook", "room_hook");
    unnamed(src, "/rooms/:room_id/:bot_key/raw/:id");
}

#[test]
fn a_route_outside_any_resource_is_named_from_the_scope_as_and_its_action() {
    // A scope path — dynamic or not — is no part of the name; only the
    // namespace or `scope as:` is. A path that is no plain word
    // (`robots.txt`, a `(.:format)` group) gets no automatic name, and a
    // dynamic one under a namespace takes just the namespace's name.
    let src = r#"Rails.application.routes.draw do
  scope path: "/v1" do
    get "ping", to: "ping#show"
  end
  scope ":locale" do
    get "about", to: "pages#about"
  end
  scope ":tenant", as: :tenant do
    get "home", to: "pages#home"
  end
  namespace :ops do
    get "health", to: "health#show"
    get "jobs/:id/retry", to: "jobs#retry"
  end
  get "robots.txt", to: "pages#robots"
  get "/feed(.:format)", to: "pages#feed"
end
"#;
    name_for(src, "/v1/ping", "ping");
    name_for(src, "/:locale/about", "about");
    name_for(src, "/:tenant/home", "tenant_home");
    name_for(src, "/ops/health", "ops_health");
    name_for(src, "/ops/jobs/:id/retry", "ops");
    no_helper(src, "v1_ping");
    unnamed(src, "/robots.txt");
    unnamed(src, "/feed");
}

#[test]
fn renamed_bare_verb_helpers_build_the_url_and_dispatch_to_the_action() {
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  resources :articles do
    resources :comments, only: [:create, :destroy]
  end
  resource :profile, only: [] do
    get :preview
    resources :links, only: [] do
      get :clicks, to: "articles#index"
    end
  end
  resources :authors, only: [] do
    resources :books, only: [] do
      get :cover, to: "articles#new"
    end
    resource :calendar, only: [] do
      get :agenda, to: "articles#index"
    end
  end
end
"#,
        )
        .write(
            "app/controllers/profiles_controller.rb",
            r#"class ProfilesController < ApplicationController
  def preview
    redirect_to agenda_author_calendar_path(3)
  end
end
"#,
        )
        .run_ruby(
            r##"
[
  [RouteHelpers.preview_profile_path, "/profile/preview", :profiles, :preview],
  [RouteHelpers.profile_link_clicks_path(7), "/profile/links/7/clicks", :articles, :index],
  [RouteHelpers.author_book_cover_path(1, 2), "/authors/1/books/2/cover", :articles, :new],
  [RouteHelpers.agenda_author_calendar_path(3), "/authors/3/calendar/agenda", :articles, :index],
].each do |path, want, controller, action|
  raise "helper built #{path}, want #{want}" unless path == want
  matched = ActionDispatch::Router.match("GET", path, RouteTable.table)
  raise "no route for #{path}" if matched.nil?
  raise "#{path}: #{matched.controller}##{matched.action}" unless
    matched.controller == controller && matched.action == action
end
status, headers, _body = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/profile/preview", "QUERY_STRING" => "",
  "rack.input" => StringIO.new("")
)
location = headers["location"] || headers["Location"]
raise "#{status} #{headers}" unless status == 302 && location.to_s.end_with?("/authors/3/calendar/agenda")
puts "PASS bare verb helpers"
"##,
        )
        .assert_passes();
}
