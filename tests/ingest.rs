//! Ingest smoke test: reading fixtures/tiny-blog/ produces the expected IR.

use std::path::Path;

use roundhouse::dialect::{Association, CallbackHook, Dependent, ValidationRule};
use roundhouse::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use roundhouse::ingest::ingest_app;
use roundhouse::schema::ColumnType;
use roundhouse::HttpMethod;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

#[test]
fn ingests_schema_tables_and_columns() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.schema.tables.len(), 2, "expected posts and comments");

    let posts = app
        .schema
        .tables
        .get(&roundhouse::Symbol::from("posts"))
        .expect("posts table");
    // Implicit id (synthesized, primary_key=true) + explicit title.
    assert_eq!(posts.columns.len(), 2);
    let id = &posts.columns[0];
    assert_eq!(id.name.as_str(), "id");
    assert!(id.primary_key);
    assert!(matches!(id.col_type, ColumnType::BigInt));
    let title = &posts.columns[1];
    assert_eq!(title.name.as_str(), "title");
    assert!(matches!(title.col_type, ColumnType::String { .. }));
    assert!(!title.nullable);

    let comments = app
        .schema
        .tables
        .get(&roundhouse::Symbol::from("comments"))
        .expect("comments table");
    // Implicit id + explicit body + explicit post_id.
    assert_eq!(comments.columns.len(), 3);
    assert_eq!(comments.columns[0].name.as_str(), "id");
    let body = &comments.columns[1];
    assert_eq!(body.name.as_str(), "body");
    assert!(matches!(body.col_type, ColumnType::Text));
    let post_id = &comments.columns[2];
    assert_eq!(post_id.name.as_str(), "post_id");
    assert!(matches!(post_id.col_type, ColumnType::BigInt));
}

#[test]
fn ingests_models_with_derived_attributes() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.models.len(), 2);

    let by_name = |n: &str| {
        app.models
            .iter()
            .find(|m| m.name.0.as_str() == n)
            .unwrap_or_else(|| panic!("no model named {n}"))
    };

    let post = by_name("Post");
    assert_eq!(post.table.0.as_str(), "posts");
    assert!(post.attributes.fields.contains_key(&roundhouse::Symbol::from("title")));

    let comment = by_name("Comment");
    assert_eq!(comment.table.0.as_str(), "comments");
    assert!(comment.attributes.fields.contains_key(&roundhouse::Symbol::from("body")));
    assert!(comment.attributes.fields.contains_key(&roundhouse::Symbol::from("post_id")));
}

#[test]
fn ingests_associations_with_convention_defaults() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    let post_assocs: Vec<&Association> = post.associations().collect();
    assert_eq!(post_assocs.len(), 1);
    match post_assocs[0] {
        Association::HasMany { name, target, foreign_key, through, dependent, scope, .. } => {
            assert_eq!(name.as_str(), "comments");
            assert_eq!(target.0.as_str(), "Comment");
            assert_eq!(foreign_key.as_str(), "post_id");
            assert!(through.is_none());
            assert!(matches!(dependent, Dependent::None));
            assert!(scope.is_none());
        }
        other => panic!("expected HasMany, got {other:?}"),
    }

    let comment = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Comment")
        .unwrap();
    let comment_assocs: Vec<&Association> = comment.associations().collect();
    assert_eq!(comment_assocs.len(), 1);
    match comment_assocs[0] {
        Association::BelongsTo { name, target, foreign_key, optional, .. } => {
            assert_eq!(name.as_str(), "post");
            assert_eq!(target.0.as_str(), "Post");
            assert_eq!(foreign_key.as_str(), "post_id");
            assert!(!optional);
        }
        other => panic!("expected BelongsTo, got {other:?}"),
    }
}

#[test]
fn ingests_validations() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    let validations: Vec<&roundhouse::Validation> = post.validations().collect();
    assert_eq!(validations.len(), 1);
    let v = validations[0];
    assert_eq!(v.attribute.as_str(), "title");
    assert_eq!(v.rules.len(), 1);
    assert!(matches!(v.rules[0], ValidationRule::Presence));
}

#[test]
fn ingests_callbacks() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    let callbacks: Vec<&roundhouse::Callback> = post.callbacks().collect();
    assert_eq!(callbacks.len(), 1);
    let cb = callbacks[0];
    assert!(matches!(cb.hook, CallbackHook::BeforeSave));
    assert_eq!(cb.targets.len(), 1);
    assert_eq!(cb.targets[0].as_str(), "normalize_title");
    assert!(cb.on.is_none());
    assert!(cb.condition.is_none());
}

#[test]
fn ingests_posts_controller_with_actions() {
    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.controllers.len(), 1);
    let ctrl = &app.controllers[0];
    assert_eq!(ctrl.name.0.as_str(), "PostsController");
    assert_eq!(
        ctrl.parent.as_ref().unwrap().0.as_str(),
        "ApplicationController"
    );
    let actions: Vec<&roundhouse::Action> = ctrl.actions().collect();
    // 5 = the 4 public scaffold actions + the private `post_params`
    // helper (`actions()` doesn't filter on Ruby visibility; it walks
    // every `def`). post_params was added 2026-05-24 alongside Phase 6
    // step 2 so the `Post.new(post_params)` rewrite finds a callee.
    assert_eq!(actions.len(), 5);
    let names: Vec<_> = actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["index", "show", "create", "destroy", "post_params"]
    );

    // index body: `@posts = Post.all` — Assign(Ivar(posts), Send(Some(Const(Post)), "all", []))
    let index = actions[0];
    match *index.body.node {
        ExprNode::Assign { ref target, ref value } => {
            match target {
                LValue::Ivar { name } => assert_eq!(name.as_str(), "posts"),
                other => panic!("expected Ivar(posts), got {other:?}"),
            }
            match *value.node {
                ExprNode::Send { recv: Some(ref recv), ref method, ref args, .. } => {
                    assert_eq!(method.as_str(), "all");
                    assert!(args.is_empty());
                    match *recv.node {
                        ExprNode::Const { ref path } => assert_eq!(path[0].as_str(), "Post"),
                        ref other => panic!("expected Const(Post), got {other:?}"),
                    }
                }
                ref other => panic!("expected Send with receiver, got {other:?}"),
            }
        }
        ref other => panic!("expected Assign, got {other:?}"),
    }

    // show body: `@post = Post.find(params[:id])`
    // Expect: Assign(Ivar(post), Send(Const(Post), "find", [Send(Send(None, "params", []), "[]", [Sym(id)])]))
    let show = actions[1];
    match *show.body.node {
        ExprNode::Assign { ref target, ref value } => {
            assert!(matches!(target, LValue::Ivar { name } if name.as_str() == "post"));
            match *value.node {
                ExprNode::Send { recv: Some(_), ref method, ref args, .. } => {
                    assert_eq!(method.as_str(), "find");
                    assert_eq!(args.len(), 1);
                    // The argument is `params[:id]` — Send(Send(None, params, []), [], [:id])
                    match *args[0].node {
                        ExprNode::Send { recv: Some(ref inner_recv), ref method, ref args, .. } => {
                            assert_eq!(method.as_str(), "[]");
                            assert_eq!(args.len(), 1);
                            // inner_recv is the implicit-self `params` call
                            match *inner_recv.node {
                                ExprNode::Send { recv: None, ref method, ref args, .. } => {
                                    assert_eq!(method.as_str(), "params");
                                    assert!(args.is_empty());
                                }
                                ref other => panic!("expected implicit-self params, got {other:?}"),
                            }
                        }
                        ref other => panic!("expected `[]` Send, got {other:?}"),
                    }
                }
                ref other => panic!("expected Send, got {other:?}"),
            }
        }
        ref other => panic!("expected Assign, got {other:?}"),
    }
}

#[test]
fn ingests_routes_file() {
    use roundhouse::RouteSpec;

    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.routes.entries.len(), 4);

    fn as_explicit(spec: &RouteSpec) -> (&HttpMethod, &str, &str, &str, Option<&str>) {
        let RouteSpec::Explicit { method, path, controller, action, as_name, .. } = spec
        else {
            panic!("expected Explicit, got {spec:?}");
        };
        (
            method,
            path.as_str(),
            controller.0.as_str(),
            action.as_str(),
            as_name.as_ref().map(|s| s.as_str()),
        )
    }

    let (m, path, ctrl, action, name) = as_explicit(&app.routes.entries[0]);
    assert!(matches!(m, HttpMethod::Get));
    assert_eq!(path, "/posts");
    assert_eq!(ctrl, "PostsController");
    assert_eq!(action, "index");
    assert_eq!(name, Some("posts"));

    let (m, path, _, action, _) = as_explicit(&app.routes.entries[1]);
    assert!(matches!(m, HttpMethod::Post));
    assert_eq!(path, "/posts");
    assert_eq!(action, "create");

    let (m, path, _, action, name) = as_explicit(&app.routes.entries[2]);
    assert!(matches!(m, HttpMethod::Get));
    assert_eq!(path, "/posts/:id");
    assert_eq!(action, "show");
    assert_eq!(name, Some("post"));

    let (m, path, _, action, _) = as_explicit(&app.routes.entries[3]);
    assert!(matches!(m, HttpMethod::Delete));
    assert_eq!(path, "/posts/:id");
    assert_eq!(action, "destroy");
}

#[test]
fn ingests_namespaced_and_split_routes() {
    use roundhouse::lower::routes::flatten_routes;

    // namespace / scope / singular resource / draw(:name) — the
    // Mastodon-class routing surface (#63 follow-up). The split file
    // is standard DSL at top level, loaded by `draw(:admin)`.
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  root "home#index"
  get "health", to: "health#show"
  scope module: :web do
    get "/embed", to: "home#embed", as: :embed
  end
  namespace :api do
    namespace :v1 do
      resources :statuses, only: [:show]
    end
  end
  draw(:admin)
end
"#,
        ),
        (
            "config/routes/admin.rb",
            r#"namespace :admin do
  get "/dashboard", to: "dashboard#index"
  resources :domain_allows, only: [:new, :create]
  resource :profile, only: [:show, :update]
end
"#,
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    let flat = flatten_routes(&app);

    let find = |path: &str, action: &str| {
        flat.iter()
            .find(|r| r.path == path && r.action.as_str() == action)
            .unwrap_or_else(|| {
                panic!(
                    "no route {path} #{action}; have {:?}",
                    flat.iter().map(|r| (&r.path, r.action.as_str())).collect::<Vec<_>>()
                )
            })
    };

    // Path without a leading slash still roots.
    assert_eq!(find("/health", "show").controller.0.as_str(), "HealthController");

    // `scope module:` qualifies the controller but not the path.
    let embed = find("/embed", "embed");
    assert_eq!(embed.controller.0.as_str(), "Web::HomeController");
    assert_eq!(embed.as_name, "embed");

    // Two nested namespaces compose path, module, and helper prefix.
    let status = find("/api/v1/statuses/:id", "show");
    assert_eq!(status.controller.0.as_str(), "Api::V1::StatusesController");
    assert_eq!(status.as_name, "api_v1_status");

    // draw(:admin) splices the split file; namespace facets apply.
    let dash = find("/admin/dashboard", "index");
    assert_eq!(dash.controller.0.as_str(), "Admin::DashboardController");
    assert_eq!(dash.as_name, "admin_dashboard");
    let new_allow = find("/admin/domain_allows/new", "new");
    assert_eq!(new_allow.controller.0.as_str(), "Admin::DomainAllowsController");
    assert_eq!(new_allow.as_name, "new_admin_domain_allow");

    // Singular resource: no :id segment, plural controller.
    let profile = find("/admin/profile", "show");
    assert_eq!(profile.controller.0.as_str(), "Admin::ProfilesController");
    assert_eq!(profile.as_name, "admin_profile");
    assert!(
        !flat.iter().any(|r| r.path.starts_with("/admin/profile/:")),
        "singular resource must not take an :id segment"
    );
}

#[test]
fn routes_recover_per_entry_under_survey() {
    // One unknown DSL entry must not zero the table: survey mode
    // records the gap and keeps the sibling routes; strict mode still
    // fails loud so fixtures force recognizers.
    let source = br#"Rails.application.routes.draw do
  use_doorkeeper
  get "/posts", to: "posts#index"
end
"#;

    let strict = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    assert!(strict.0.is_err(), "strict ingest fails loud on unknown DSL");

    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    let table = result.expect("survey ingest recovers");
    assert_eq!(table.entries.len(), 1, "the good route survives");
    assert!(
        gaps.iter().any(|g| format!("{g:?}").contains("use_doorkeeper")),
        "the unknown-DSL gap is recorded, not silently dropped: {gaps:?}"
    );
}

#[test]
fn devise_scope_and_authenticated_passthrough_nested_routes() {
    let source = br#"Rails.application.routes.draw do
  authenticated :user, lambda { |u| u.admin? } do
    namespace :admin do
      resources :users, only: [:index]
      root to: "dashboard#show"
    end
  end
  unauthenticated :user do
    get "join", to: "registrations#new"
  end
  devise_scope :user do
    get "session/otp", to: "sessions#otp"
  end
  authenticated :user do
    root to: "dashboard#show", as: :user_root
  end
end
"#;
    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    let table = result.expect("ingest");
    assert!(
        gaps.iter().all(|g| {
            let s = format!("{g:?}");
            !s.contains("authenticated")
                && !s.contains("unauthenticated")
                && !s.contains("devise_scope")
        }),
        "Devise wrappers must not survey: {gaps:?}"
    );
    let mut app = roundhouse::App::default();
    app.routes = table;
    let flat = roundhouse::lower::flatten_routes(&app);
    assert!(
        flat.iter().any(|r| r.path == "/admin/users" && r.as_name == "admin_users"),
        "authenticated nested resources: {flat:?}"
    );
    assert!(
        flat.iter().any(|r| r.path == "/join" && r.controller.0.as_str() == "RegistrationsController"),
        "unauthenticated nested route: {flat:?}"
    );
    assert!(
        flat.iter().any(|r| r.path == "/session/otp" && r.controller.0.as_str() == "SessionsController"),
        "devise_scope nested route: {flat:?}"
    );
    assert!(
        flat.iter().any(|r| r.as_name == "user_root"),
        "authenticated root as: :user_root: {flat:?}"
    );
    assert!(
        flat.iter().any(|r| r.path == "/admin" && r.as_name == "admin_root"),
        "namespaced root without as: keeps admin_root: {flat:?}"
    );
}

#[test]
fn namespaced_root_as_applies_name_prefix() {
    let source = br#"Rails.application.routes.draw do
  namespace :admin do
    root to: "dashboard#show", as: :home
  end
end
"#;
    let table = roundhouse::ingest::ingest_routes(source, "config/routes.rb").expect("ingest");
    let mut app = roundhouse::App::default();
    app.routes = table;
    let flat = roundhouse::lower::flatten_routes(&app);
    assert!(
        flat.iter().any(|r| r.path == "/admin" && r.as_name == "admin_home"),
        "namespace as: prefix on root: {flat:?}"
    );
    assert!(
        !flat.iter().any(|r| r.as_name == "home"),
        "bare home helper must not win over admin_home: {flat:?}"
    );
}

#[test]
fn devise_for_expands_session_and_registration_helpers() {
    let source = br#"Rails.application.routes.draw do
  devise_for :users,
    controllers: {
      registrations: "users/registrations",
      sessions: "users/sessions"
    }
end
"#;
    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    let table = result.expect("ingest");
    assert!(
        gaps.iter().all(|g| !format!("{g:?}").contains("devise_for")),
        "devise_for must not survey: {gaps:?}"
    );
    let mut app = roundhouse::App::default();
    app.routes = table;
    let flat = roundhouse::lower::flatten_routes(&app);
    let by_name: std::collections::HashMap<_, _> =
        flat.iter().map(|r| (r.as_name.as_str(), r)).collect();
    let session = by_name.get("new_user_session").expect("new_user_session");
    assert_eq!(session.path, "/users/sign_in");
    assert_eq!(session.controller.0.as_str(), "Users::SessionsController");
    let reg = by_name.get("new_user_registration").expect("new_user_registration");
    assert_eq!(reg.path, "/users/sign_up");
    assert_eq!(reg.controller.0.as_str(), "Users::RegistrationsController");
    assert!(by_name.contains_key("destroy_user_session"));
    assert!(by_name.contains_key("edit_user_password"));
    assert!(by_name.contains_key("user_confirmation"));
}

#[test]
fn devise_for_accepts_string_controller_keys() {
    // Devise accepts string keys in `controllers:`; skipping them would
    // silently fall back to Devise::*Controller.
    let source = br#"Rails.application.routes.draw do
  devise_for :users,
    controllers: {
      "sessions" => "users/sessions",
      registrations: "users/registrations"
    }
end
"#;
    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    let table = result.expect("ingest");
    assert!(
        gaps.iter().all(|g| !format!("{g:?}").contains("devise_for")),
        "string-keyed controllers must not survey: {gaps:?}"
    );
    let mut app = roundhouse::App::default();
    app.routes = table;
    let flat = roundhouse::lower::flatten_routes(&app);
    let by_name: std::collections::HashMap<_, _> =
        flat.iter().map(|r| (r.as_name.as_str(), r)).collect();
    let session = by_name.get("new_user_session").expect("new_user_session");
    assert_eq!(session.controller.0.as_str(), "Users::SessionsController");
    let reg = by_name.get("new_user_registration").expect("new_user_registration");
    assert_eq!(reg.controller.0.as_str(), "Users::RegistrationsController");
}

#[test]
fn devise_for_rejects_unmodeled_controller_mappings() {
    // OmniAuth (and any mapping outside the static four) must fail loud —
    // accepting the key then emitting no routes would hide the gap.
    let source = br#"Rails.application.routes.draw do
  devise_for :users, controllers: { omniauth_callbacks: "users/omniauth_callbacks" }
end
"#;
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let err = result.expect_err("unmodeled controllers: key must fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("omniauth_callbacks") || msg.contains("unsupported devise_for controllers"),
        "expected unmodeled mapping error, got: {msg}"
    );
}

#[test]
fn devise_for_defaults_controllers_under_devise_module() {
    // Bare `devise_for :users` must resolve to Devise::*Controller, not
    // top-level SessionsController (Devise::Mapping#default_controllers).
    let source = br#"Rails.application.routes.draw do
  devise_for :users
end
"#;
    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    let table = result.expect("ingest");
    assert!(
        gaps.iter().all(|g| !format!("{g:?}").contains("devise_for")),
        "bare devise_for must not survey: {gaps:?}"
    );
    let mut app = roundhouse::App::default();
    app.routes = table;
    let flat = roundhouse::lower::flatten_routes(&app);
    let by_name: std::collections::HashMap<_, _> =
        flat.iter().map(|r| (r.as_name.as_str(), r)).collect();
    let session = by_name.get("new_user_session").expect("new_user_session");
    assert_eq!(session.path, "/users/sign_in");
    assert_eq!(session.controller.0.as_str(), "Devise::SessionsController");
    let reg = by_name.get("new_user_registration").expect("new_user_registration");
    assert_eq!(reg.controller.0.as_str(), "Devise::RegistrationsController");
    let password = by_name.get("new_user_password").expect("new_user_password");
    assert_eq!(password.controller.0.as_str(), "Devise::PasswordsController");
    let confirmation = by_name.get("user_confirmation").expect("user_confirmation");
    assert_eq!(
        confirmation.controller.0.as_str(),
        "Devise::ConfirmationsController"
    );
}

/// A route omission is an error on the recovered table, not a fatal parse.
#[test]
fn routes_mount_diagnostic_preserves_siblings_in_every_mode() {
    let source = br#"Rails.application.routes.draw do
  mount Sidekiq::Web, at: "sidekiq"
  get "/posts", to: "posts#index"
end
"#;
    let strict = roundhouse::ingest::ingest_routes(source, "config/routes.rb")
        .expect("mount diagnostics do not abort ingest");
    assert_eq!(strict.entries.len(), 1);
    assert_eq!(strict.diagnostics.len(), 1);
    let diagnostic = &strict.diagnostics[0];
    assert_eq!(diagnostic.severity, roundhouse::diagnostic::Severity::Error);
    assert!(!diagnostic.span.is_synthetic());
    assert_eq!(&source[diagnostic.span.start as usize..diagnostic.span.end as usize],
        br#"mount Sidekiq::Web, at: "sidekiq""#);

    roundhouse::ingest::survey::activate();
    let result = roundhouse::ingest::ingest_routes(source, "config/routes.rb");
    let gaps = roundhouse::ingest::survey::drain();
    let surveyed = result.expect("survey ingest succeeds");
    assert_eq!(surveyed.entries, strict.entries);
    assert_eq!(surveyed.diagnostics, strict.diagnostics);
    assert_eq!(gaps.len(), 1, "one survey ledger entry: {gaps:?}");
}

/// Draw files retain their own source attribution and share mount recovery.
#[test]
fn mounts_in_split_route_files_keep_the_split_file_span() {
    let table = roundhouse::ingest::routes::ingest_routes_with_draws(
        b"Rails.application.routes.draw do\n  draw :admin\nend\n",
        "config/routes.rb",
        &std::collections::HashMap::from([("admin".to_string(), (
            b"mount Catalog::Engine, at: '/catalog'\nget '/ok', to: 'posts#index'\n".to_vec(),
            "config/routes/admin.rb".to_string(),
        ))]),
    ).unwrap();
    assert_eq!(table.entries.len(), 1);
    assert_eq!(table.diagnostics.len(), 1);
    assert_eq!(table.diagnostics[0].span.file,
        roundhouse::ingest::sources::file_id("config/routes/admin.rb"));
}

/// A top-level draw is transparent to the fixed runtime cable mount.
#[test]
fn top_level_draw_preserves_runtime_cable_mount_context() {
    let table = roundhouse::ingest::routes::ingest_routes_with_draws(
        b"Rails.application.routes.draw do\n  draw :cable\nend\n",
        "config/routes.rb",
        &std::collections::HashMap::from([("cable".to_string(), (
            b"mount ActionCable.server => '/cable'\n".to_vec(),
            "config/routes/cable.rb".to_string(),
        ))]),
    ).unwrap();
    assert!(table.diagnostics.is_empty(), "{table:?}");
}

/// Transparent draw/concern expansion inherits its invocation's mount scope.
#[test]
fn cable_mounts_inherit_draw_and_concern_scope() {
    let draws = std::collections::HashMap::from([("cable".to_string(), (
        b"mount ActionCable.server => '/cable'\n".to_vec(),
        "config/routes/cable.rb".to_string(),
    ))]);
    for (body, expected) in [
        ("draw :cable", 0),
        ("namespace :admin do\n draw :cable\nend", 1),
        ("concern :live do\n mount ActionCable.server => '/cable'\nend\nconcerns :live", 0),
        ("concern :live do\n mount ActionCable.server => '/cable'\nend\nnamespace :admin do\n concerns :live\nend", 1),
        ("concern :live do\n mount ActionCable.server => '/cable'\nend\nresources :widgets, concerns: :live", 1),
        ("constraints id: /[0-9]+/ do\n mount ActionCable.server => '/cable'\nend", 1),
        ("authenticated :user do\n mount ActionCable.server => '/cable'\nend", 1),
        ("unauthenticated :user do\n mount ActionCable.server => '/cable'\nend", 1),
        ("devise_scope :user do\n mount ActionCable.server => '/cable'\nend", 1),
        ("if Rails.env.development?\n mount ActionCable.server => '/cable'\nend", 1),
    ] {
        let source = format!("Rails.application.routes.draw do\n{body}\nend\n");
        let table = roundhouse::ingest::routes::ingest_routes_with_draws(
            source.as_bytes(), "config/routes.rb", &draws,
        ).unwrap();
        assert_eq!(table.diagnostics.len(), expected, "{body}: {table:?}");
        for diagnostic in &table.diagnostics {
            let span = diagnostic.span;
            let origin = if span.file == roundhouse::ingest::sources::file_id("config/routes/cable.rb") {
                draws["cable"].0.as_slice()
            } else {
                source.as_bytes()
            };
            assert_eq!(&origin[span.start as usize..span.end as usize],
                b"mount ActionCable.server => '/cable'", "{body}: located mount");
        }
    }
}

/// A loaded file's own draw opens a fresh mapper, then restores the caller's
/// scope. Draw inclusion instead keeps the caller's mapper.
#[test]
fn loaded_route_draw_resets_and_restores_mount_scope() {
    let loaded = b"Rails.application.routes.draw do\n mount ActionCable.server => '/cable'\n mount Catalog::Engine, at: '/catalog'\n get '/ok', to: 'posts#index'\nend\n";
    for include in [
        "load Rails.root.join('config/routes/cable.rb')",
        "instance_eval(File.read(Rails.root.join('config/routes/cable.rb')))",
    ] {
        let source = format!("Rails.application.routes.draw do\n namespace :admin do\n  {include}\n  mount ActionCable.server => '/cable'\n end\n mount ActionCable.server => '/cable'\nend\n");
        let table = roundhouse::ingest::routes::ingest_routes_with_draws(
            source.as_bytes(), "config/routes.rb",
            &std::collections::HashMap::from([("cable".to_string(), (
                loaded.to_vec(), "config/routes/cable.rb".to_string(),
            ))]),
        ).unwrap();
        assert_eq!(table.diagnostics.len(), 2, "{include}: {table:?}");
        assert_eq!(table.diagnostics[0].span.file,
            roundhouse::ingest::sources::file_id("config/routes/cable.rb"));
        let span = table.diagnostics[0].span;
        assert_eq!(&loaded[span.start as usize..span.end as usize],
            b"mount Catalog::Engine, at: '/catalog'");
        assert_eq!(table.diagnostics[1].span.file,
            roundhouse::ingest::sources::file_id("config/routes.rb"));
        assert!(table.entries.iter().any(|r| matches!(r,
            roundhouse::RouteSpec::Explicit { path, .. } if path == "/ok")),
            "the loaded sibling stays at top scope: {table:?}");
    }
}

#[test]
fn ingested_app_is_self_consistent() {
    use roundhouse::RouteSpec;

    let app = ingest_app(fixture_path()).expect("ingest");
    assert_eq!(app.schema_version, roundhouse::App::SCHEMA_VERSION);
    // Serialize / deserialize proves the ingested shape is round-trippable.
    let json = serde_json::to_string_pretty(&app).expect("serialize");
    let _: roundhouse::App = serde_json::from_str(&json).expect("deserialize");
    // Make sure the Rails dependency between route and controller is intact.
    let ctrl_names: Vec<_> = app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    for entry in &app.routes.entries {
        if let RouteSpec::Explicit { controller, .. } = entry {
            assert!(
                ctrl_names.contains(&controller.0.as_str()),
                "route references unknown controller {:?}",
                controller
            );
        }
    }
}

#[test]
fn literal_ingested_expr() {
    let source = br#"42"#;
    let result = ruby_prism::parse(source);
    let program = result.node();
    let prog = program.as_program_node().unwrap();
    let stmt = prog.statements().body().iter().next().unwrap();
    let expr = roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap();
    match *expr.node {
        ExprNode::Lit { value: Literal::Int { value } } => assert_eq!(value, 42),
        ref other => panic!("expected Lit(Int 42), got {other:?}"),
    }
    let _ = Expr::new(expr.span, *expr.node); // just making sure imports are alive
}

#[test]
fn special_variable_reads_ingest_as_sigil_named_vars() {
    // `@@classvar`, `$global`, and `$&` (back-reference) each ingest as a
    // `Var` whose name keeps the sigil verbatim — the same convention the
    // numbered-reference (`$1`) handler uses. Without these, a single such
    // read in a support class (e.g. Keybase's `@@config`, Sponge's
    // `$stdout`) fails ingest and, under per-file isolation, drops every
    // method on the class so external calls fall to "no known method".
    fn read_var_name(source: &[u8]) -> String {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        let expr = roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap();
        match *expr.node {
            ExprNode::Var { name, .. } => name.as_str().to_string(),
            ref other => panic!("expected Var, got {other:?}"),
        }
    }
    assert_eq!(read_var_name(b"@@config"), "@@config");
    assert_eq!(read_var_name(b"$stdout"), "$stdout");
    // A back-reference is set by a preceding match; parse it in context so
    // prism produces a BackReferenceReadNode rather than a plain global.
    let result = ruby_prism::parse(b"\"x\" =~ /x/; $&");
    let program = result.node();
    let prog = program.as_program_node().unwrap();
    let stmt = prog.statements().body().iter().nth(1).unwrap();
    let expr = roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap();
    match *expr.node {
        ExprNode::Var { name, .. } => assert_eq!(name.as_str(), "$&"),
        ref other => panic!("expected Var($&), got {other:?}"),
    }
}

#[test]
fn retry_and_redo_ingest_and_round_trip_through_ruby() {
    // `retry` (inside a rescue body) and `redo` (inside a block) ingest as
    // the value-less divergent nodes `ExprNode::Retry` / `ExprNode::Redo`
    // and round-trip verbatim through the Ruby emitter.
    use roundhouse::emit::ruby::emit_expr;

    fn ingest_first(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
    }

    // Depth-first search for any node satisfying `pred`.
    fn any_node(e: &Expr, pred: &dyn Fn(&ExprNode) -> bool) -> bool {
        if pred(&e.node) {
            return true;
        }
        let mut found = false;
        e.node.for_each_child(&mut |c| {
            if any_node(c, pred) {
                found = true;
            }
        });
        found
    }

    let with_retry = ingest_first(b"begin\n  foo\nrescue\n  retry\nend");
    assert!(
        any_node(&with_retry, &|n| matches!(n, ExprNode::Retry)),
        "expected an ExprNode::Retry; got {:?}",
        with_retry.node
    );
    assert!(
        emit_expr(&with_retry).contains("retry"),
        "Ruby emit should keep `retry`; got:\n{}",
        emit_expr(&with_retry)
    );

    let with_redo = ingest_first(b"[1].each do |x|\n  redo\nend");
    assert!(
        any_node(&with_redo, &|n| matches!(n, ExprNode::Redo)),
        "expected an ExprNode::Redo; got {:?}",
        with_redo.node
    );
    assert!(
        emit_expr(&with_redo).contains("redo"),
        "Ruby emit should keep `redo`; got:\n{}",
        emit_expr(&with_redo)
    );
}

#[test]
fn modern_ruby_syntax_desugars() {
    use roundhouse::emit::ruby::emit_expr;

    // Ruby 3.1 keyword punning: `f(short_id:)` reads the same-named
    // local/method (prism ImplicitNode) — desugars to an explicit pair.
    let punned = ingest_snippet(b"find_by!(short_id:)");
    assert!(
        emit_expr(&punned).contains("short_id: short_id"),
        "punned kwarg expands to an explicit pair; got:\n{}",
        emit_expr(&punned)
    );

    // Ruby 3.4 `it` implicit block parameter — becomes a real |it| param.
    let with_it = ingest_snippet(b"[1].map { it + 1 }");
    let emitted = emit_expr(&with_it);
    assert!(
        emitted.contains("|it|") && emitted.contains("it + 1"),
        "`it` block gains an explicit |it| param; got:\n{emitted}"
    );

    // Interpolated symbol — string interpolation sent `.to_sym`.
    let interp_sym = ingest_snippet(b":\"#{name}_id\"");
    assert!(
        emit_expr(&interp_sym).contains(".to_sym"),
        "interpolated symbol desugars via to_sym; got:\n{}",
        emit_expr(&interp_sym)
    );

    // `/o` once-flag interp regex — flag dropped, plain Regexp.new.
    let once_re = ingest_snippet(b"/^\\$2a\\$#{cost}\\$/o");
    assert!(
        emit_expr(&once_re).contains("Regexp.new"),
        "once-flag interp regex still desugars to Regexp.new; got:\n{}",
        emit_expr(&once_re)
    );
}

fn ingest_snippet(source: &[u8]) -> Expr {
    let result = ruby_prism::parse(source);
    let program = result.node();
    let prog = program.as_program_node().unwrap();
    let stmt = prog.statements().body().iter().next().unwrap();
    roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
}

#[test]
fn interpolated_regex_with_flags_carries_options() {
    // `/…#{…}…/i` desugars to `Regexp.new(<interp>, options)` where the
    // options integer carries the i/m/x bits (IGNORECASE=1 here).
    let expr = ingest_snippet(b"/^X-BeenThere: #{shortname}-/i");
    match &*expr.node {
        ExprNode::Send { recv, method, args, .. } => {
            assert!(
                matches!(recv.as_ref().map(|r| &*r.node), Some(ExprNode::Const { .. })),
                "receiver should be the Regexp const"
            );
            assert_eq!(method.as_str(), "new");
            assert_eq!(args.len(), 2, "pattern + options arg; got {args:?}");
            match &*args[1].node {
                ExprNode::Lit { value: Literal::Int { value } } => assert_eq!(*value, 1),
                other => panic!("expected Int options arg, got {other:?}"),
            }
        }
        other => panic!("expected Regexp.new Send, got {other:?}"),
    }

    // A flag-free interp regex stays single-arg (no options appended).
    let plain = ingest_snippet(b"/^#{shortname}-/");
    match &*plain.node {
        ExprNode::Send { args, .. } => assert_eq!(args.len(), 1, "no options arg expected"),
        other => panic!("expected Regexp.new Send, got {other:?}"),
    }
}

#[test]
fn multi_write_index_target_ingests_as_index_lvalue() {
    // `recv[k], a, b = rhs` — an index write used as a parallel-assignment
    // target (lobsters markdowner.rb:82).
    let expr = ingest_snippet(b"link['href'], title, alt = attrs");
    match &*expr.node {
        ExprNode::MultiAssign { targets, .. } => {
            assert_eq!(targets.len(), 3);
            assert!(
                matches!(&targets[0], LValue::Index { .. }),
                "first target should be Index, got {:?}",
                targets[0]
            );
            assert!(matches!(&targets[1], LValue::Var { .. }));
            assert!(matches!(&targets[2], LValue::Var { .. }));
        }
        other => panic!("expected MultiAssign, got {other:?}"),
    }
}

#[test]
fn block_delimiter_style_is_preserved() {
    use roundhouse::{BlockStyle, ExprNode, ModelBodyItem};

    // Two consecutive class-body calls with different block delimiters —
    // the Ruby emitter uses the preserved style to pick `{ }` vs do…end.
    let source = br#"
class Widget < ApplicationRecord
  after_create_commit { ping }
  after_destroy_commit do
    pong
  end
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema, &Default::default())
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 2);

    fn block_style_of(item: &ModelBodyItem) -> BlockStyle {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            panic!("expected Unknown body item, got {item:?}");
        };
        let ExprNode::Send { block: Some(block), .. } = &*expr.node else {
            panic!("expected Send-with-block");
        };
        let ExprNode::Lambda { block_style, .. } = &*block.node else {
            panic!("expected Lambda block");
        };
        *block_style
    }

    assert!(matches!(block_style_of(&model.body[0]), BlockStyle::Brace));
    assert!(matches!(block_style_of(&model.body[1]), BlockStyle::Do));
}

#[test]
fn leading_comments_attach_to_class_body_items() {
    use roundhouse::ModelBodyItem;

    let source = br#"
class Widget < ApplicationRecord
  # This comment should attach to has_many below.
  has_many :gears

  # Two comment lines
  # both attach to validates
  validates :name, presence: true
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema, &Default::default())
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 2);

    let ModelBodyItem::Association { leading_comments: has_many_comments, .. } = &model.body[0]
    else {
        panic!("expected Association first");
    };
    assert_eq!(has_many_comments.len(), 1);
    assert_eq!(
        has_many_comments[0].text.as_str(),
        "# This comment should attach to has_many below."
    );

    let ModelBodyItem::Validation { leading_comments: validates_comments, .. } = &model.body[1]
    else {
        panic!("expected Validation second");
    };
    assert_eq!(validates_comments.len(), 2);
    assert_eq!(validates_comments[0].text.as_str(), "# Two comment lines");
    assert_eq!(
        validates_comments[1].text.as_str(),
        "# both attach to validates"
    );
}

#[test]
fn length_validation_rule_is_ingested() {
    use roundhouse::{ModelBodyItem, ValidationRule};

    let source = br#"
class Widget < ApplicationRecord
  validates :body, presence: true, length: { minimum: 10 }
  validates :title, length: { maximum: 80 }
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema, &Default::default())
        .unwrap()
        .unwrap();

    let validations: Vec<_> = model.body.iter().filter_map(|item| match item {
        ModelBodyItem::Validation { validation, .. } => Some(validation),
        _ => None,
    }).collect();

    // `validates :body, presence: true, length: { minimum: 10 }` expands
    // to one Validation for :body with two rules.
    assert_eq!(validations.len(), 2);
    let body_v = validations.iter().find(|v| v.attribute.as_str() == "body").unwrap();
    assert_eq!(body_v.rules.len(), 2);
    assert!(body_v.rules.iter().any(|r| matches!(r, ValidationRule::Presence)));
    assert!(body_v.rules.iter().any(
        |r| matches!(r, ValidationRule::Length { min: Some(10), max: None, .. })
    ));

    let title_v = validations.iter().find(|v| v.attribute.as_str() == "title").unwrap();
    assert_eq!(title_v.rules.len(), 1);
    assert!(matches!(
        title_v.rules[0],
        ValidationRule::Length { min: None, max: Some(80), .. }
    ));
}

#[test]
fn model_body_preserves_source_order_with_unknown_fallback() {
    use roundhouse::ModelBodyItem;

    // Exercise the ingest: a model with a known association, an unknown
    // class-body call (`broadcasts_to`), and a validation — in that
    // order. The body Vec must mirror that exact order, with
    // broadcasts_to captured as `Unknown` (preserved as an Expr rather
    // than silently dropped).
    let source = br#"
class Widget < ApplicationRecord
  has_many :gears
  broadcasts_to :widgets
  validates :name, presence: true
end
"#;
    let schema = roundhouse::schema::Schema::default();
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema, &Default::default())
        .unwrap()
        .unwrap();

    assert_eq!(model.body.len(), 3);
    assert!(matches!(model.body[0], ModelBodyItem::Association { .. }));
    match &model.body[1] {
        ModelBodyItem::Unknown { expr, .. } => {
            // `broadcasts_to :widgets` → Send with no receiver, method
            // "broadcasts_to", one symbol arg.
            let ExprNode::Send { method, .. } = &*expr.node else {
                panic!("expected Send, got {:?}", expr.node);
            };
            assert_eq!(method.as_str(), "broadcasts_to");
        }
        other => panic!("expected Unknown(broadcasts_to), got {other:?}"),
    }
    assert!(matches!(model.body[2], ModelBodyItem::Validation { .. }));
}

#[test]
fn string_interpolation() {
    fn parse_one(source: &[u8]) -> roundhouse::expr::Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    let e = parse_one(br#""article_#{@article.id}_comments""#);
    match &*e.node {
        ExprNode::StringInterp { parts } => {
            assert_eq!(parts.len(), 3);
            match &parts[0] {
                InterpPart::Text { value } => assert_eq!(value.as_str(), "article_"),
                other => panic!("part 0: expected Text, got {other:?}"),
            }
            match &parts[1] {
                InterpPart::Expr { expr } => {
                    // `@article.id` → Send(recv=Ivar(article), method=id)
                    assert!(matches!(&*expr.node, ExprNode::Send { .. }));
                }
                other => panic!("part 1: expected Expr, got {other:?}"),
            }
            match &parts[2] {
                InterpPart::Text { value } => assert_eq!(value.as_str(), "_comments"),
                other => panic!("part 2: expected Text, got {other:?}"),
            }
        }
        other => panic!("expected StringInterp, got {other:?}"),
    }
}

#[test]
fn array_literal_styles() {
    use roundhouse::expr::ArrayStyle;

    fn parse_one(source: &[u8]) -> roundhouse::expr::Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    // Bracket form with symbol elements.
    let e = parse_one(br"[:a, :b, :c]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::Brackets));
            assert_eq!(elements.len(), 3);
            for el in elements {
                assert!(matches!(&*el.node, ExprNode::Lit { value: Literal::Sym { .. } }));
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }

    // %i[ ... ] symbol-list form.
    let e = parse_one(br"%i[show edit update]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::PercentI));
            assert_eq!(elements.len(), 3);
            match &*elements[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    assert_eq!(value.as_str(), "show");
                }
                other => panic!("expected Sym, got {other:?}"),
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }

    // %w[ ... ] word-list form.
    let e = parse_one(br"%w[alpha beta]");
    match &*e.node {
        ExprNode::Array { elements, style } => {
            assert!(matches!(style, ArrayStyle::PercentW));
            assert_eq!(elements.len(), 2);
            match &*elements[0].node {
                ExprNode::Lit { value: Literal::Str { value } } => {
                    assert_eq!(value.as_str(), "alpha");
                }
                other => panic!("expected Str, got {other:?}"),
            }
        }
        other => panic!("expected Array, got {other:?}"),
    }
}

#[test]
fn classifies_models_vs_library_classes() {
    // The transpiled_blog fixture pairs Article (extends ApplicationRecord)
    // and ArticleCommentsProxy (no superclass) under app/models/. The
    // classifier must route the two through different paths.
    let app =
        ingest_app(Path::new("runtime/ruby/test/fixtures/transpiled_blog")).expect("ingest");

    let model_names: Vec<&str> =
        app.models.iter().map(|m| m.name.0.as_str()).collect();
    assert!(
        model_names.contains(&"Article"),
        "Article should be classified as a model, got models={model_names:?}"
    );
    assert!(
        model_names.contains(&"Comment"),
        "Comment should be classified as a model, got models={model_names:?}"
    );

    let lib_names: Vec<&str> =
        app.library_classes.iter().map(|lc| lc.name.0.as_str()).collect();
    assert_eq!(
        lib_names,
        vec!["ArticleCommentsProxy"],
        "ArticleCommentsProxy should be the lone library class"
    );

    // Library class carries `include Enumerable`, picked up by ingest.
    let proxy = &app.library_classes[0];
    let include_names: Vec<&str> =
        proxy.includes.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(include_names, vec!["Enumerable"]);

    // Methods present on the proxy: initialize, to_a, each, size,
    // empty?, build, create. (length/count are aliases, not defs.)
    let method_names: Vec<&str> =
        proxy.methods.iter().map(|m| m.name.as_str()).collect();
    for expected in ["initialize", "to_a", "each", "size", "empty?", "build", "create"] {
        assert!(
            method_names.contains(&expected),
            "expected method {expected} on ArticleCommentsProxy, got {method_names:?}"
        );
    }
}

/// Survey-mode ingest must recover from an unsupported construct (rather
/// than aborting the whole app) and must record skipped view templates.
/// This is the behavior the LSP/MCP rely on to stay usable on real apps,
/// and the surfacing that keeps unsupported (RABL/`.text.erb`/`.ruby`)
/// views from vanishing silently.
#[test]
fn survey_mode_recovers_from_unsupported_construct_and_records_skipped_views() {
    use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[
        // A backtick command (`XStringNode`) — an expression roundhouse
        // doesn't model, so it aborts strict ingest.
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @out = `echo hi`\n  end\nend\n",
        ),
        // A HAML view: now ingested through the shared view pipeline.
        ("app/views/widgets/show.html.haml", "%h1= @widget.name\n"),
        // A RABL view: still an unsupported engine the analyzer skips.
        ("app/views/widgets/index.html.rabl", "object @widget\n"),
    ];
    let tree = || -> HashMap<PathBuf, Vec<u8>> {
        files
            .iter()
            .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
            .collect()
    };

    // Strict mode (default): the unsupported construct aborts ingest.
    assert!(
        ingest_app_from_tree(tree()).is_err(),
        "expected strict ingest to abort on the unsupported backtick command"
    );

    // Survey mode: ingest recovers to a best-effort App and records every
    // gap — both the unsupported construct and the skipped HAML view.
    survey::activate();
    let result = ingest_app_from_tree(tree());
    let gaps = survey::drain();
    let app = result.expect("survey-mode ingest should recover, not abort");

    // The HAML view is now ingested rather than skipped.
    assert!(
        app.views.iter().any(|v| v.name.as_str() == "widgets/show"),
        "HAML view should be ingested through the shared view pipeline"
    );

    let messages: Vec<String> = gaps
        .iter()
        .map(|g| match g {
            IngestError::Unsupported { message, .. } => message.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("view template not ingested: rabl")),
        "skipped RABL view should be recorded as a gap, got: {messages:?}"
    );
    assert!(
        !messages.is_empty(),
        "unsupported backtick command should be recorded as a gap"
    );
}

/// The spelled-out `lambda { |x| … }` scope form (Mastodon's multi-line
/// scopes) must ingest identically to the arrow form `->(x) { … }`.
#[test]
fn spelled_lambda_scope_ingests_like_arrow_form() {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[(
        "app/models/widget.rb",
        concat!(
            "class Widget < ApplicationRecord\n",
            "  scope :arrow, ->(limit) { where(id: limit) }\n",
            "  scope :spelled, lambda { |limit| where(id: limit) }\n",
            "  scope :spelled_proc, proc { where(active: true) }\n",
            "end\n",
        ),
    )];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();

    let app = ingest_app_from_tree(tree).expect("spelled lambda scopes ingest strict");
    let widget = &app.models[0];
    let scopes: Vec<&str> = widget.scopes().map(|s| s.name.as_str()).collect();
    assert_eq!(scopes, vec!["arrow", "spelled", "spelled_proc"]);
    let spelled = widget.scopes().find(|s| s.name.as_str() == "spelled").unwrap();
    assert_eq!(spelled.params.len(), 1, "block params carry over: |limit|");
    assert_eq!(spelled.params[0].name.as_str(), "limit");
}

/// Procore writes `scope :x, ->(direction) do … end` (the `do…end`
/// arrow-lambda body, e.g. `components/instructions/app/models/
/// site_instruction.rb`) and `->(direction = :asc) { … }` (a defaulted
/// param, e.g. `components/tasks/app/models/task_item.rb`). Both are
/// still `LambdaNode`s — only the body delimiter or the parameter
/// default differs from the already-supported `-> { … }` — so both
/// must ingest exactly like the brace form.
#[test]
fn arrow_lambda_scope_do_end_and_defaulted_param_ingest() {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[(
        "app/models/widget.rb",
        concat!(
            "class Widget < ApplicationRecord\n",
            "  scope :order_by_title, ->(direction) do\n",
            "    order(\"title #{direction}\")\n",
            "  end\n",
            "  scope :order_by_kind, ->(direction = :asc) { order(kind: direction) }\n",
            "end\n",
        ),
    )];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();

    let app = ingest_app_from_tree(tree).expect("do-end and defaulted-param scopes ingest strict");
    let widget = &app.models[0];
    let scopes: Vec<&str> = widget.scopes().map(|s| s.name.as_str()).collect();
    assert_eq!(scopes, vec!["order_by_title", "order_by_kind"]);

    let do_end = widget.scopes().find(|s| s.name.as_str() == "order_by_title").unwrap();
    assert_eq!(do_end.params.len(), 1);
    assert_eq!(do_end.params[0].name.as_str(), "direction");

    let defaulted = widget.scopes().find(|s| s.name.as_str() == "order_by_kind").unwrap();
    assert_eq!(defaulted.params.len(), 1);
    assert_eq!(defaulted.params[0].name.as_str(), "direction");
    assert!(
        defaulted.params[0].default.is_some(),
        "defaulted lambda param must carry its default"
    );
}

/// `scope :for_tools, (lambda do |tools| … end)` — Procore's
/// `components/reports/app/models/report.rb` wraps the spelled-out
/// `lambda do … end` (and `proc`) form in its own parens. Before this
/// fix, the parens (a `ParenthesesNode`) sat between `parse_scope` and
/// the `lambda`/`proc` call it was looking for, so `for_tools`,
/// `for_data_sets`, and `shared` all failed with "scope body must be a
/// lambda" and killed report.rb's ingest under strict mode.
#[test]
fn parenthesized_lambda_scope_ingests() {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[(
        "app/models/widget.rb",
        concat!(
            "class Widget < ApplicationRecord\n",
            "  scope :for_tools, (lambda do |tools|\n",
            "    where('tool_type IN (?)', tools)\n",
            "  end)\n",
            "  scope :for_data_sets, (lambda do\n",
            "    joins('INNER JOIN report_tabs')\n",
            "  end)\n",
            "  scope :active, (proc { where(active: true) })\n",
            "end\n",
        ),
    )];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();

    let app = ingest_app_from_tree(tree).expect("parenthesized lambda/proc scopes ingest strict");
    let widget = &app.models[0];
    let scopes: Vec<&str> = widget.scopes().map(|s| s.name.as_str()).collect();
    assert_eq!(scopes, vec!["for_tools", "for_data_sets", "active"]);

    let for_tools = widget.scopes().find(|s| s.name.as_str() == "for_tools").unwrap();
    assert_eq!(for_tools.params.len(), 1);
    assert_eq!(for_tools.params[0].name.as_str(), "tools");

    let for_data_sets = widget.scopes().find(|s| s.name.as_str() == "for_data_sets").unwrap();
    assert_eq!(for_data_sets.params.len(), 0);
}

/// Survey mode recovers at body-item granularity: one unsupported item
/// (a scope whose body isn't a lambda in any spelling) records a gap and
/// is skipped, while the rest of the class — and the class itself —
/// survives. Before this, a single such item silently dropped the whole
/// model (Mastodon lost `Status` this way). Strict mode still aborts.
#[test]
fn survey_mode_keeps_the_class_when_one_body_item_is_unsupported() {
    use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[(
        "app/models/widget.rb",
        concat!(
            "class Widget < ApplicationRecord\n",
            "  has_many :parts\n",
            "  scope :broken, :not_a_lambda\n",
            "  scope :fine, -> { where(active: true) }\n",
            "end\n",
        ),
    )];
    let tree = || -> HashMap<PathBuf, Vec<u8>> {
        files
            .iter()
            .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
            .collect()
    };

    // Strict mode: the unsupported scope body aborts ingest.
    assert!(ingest_app_from_tree(tree()).is_err(), "strict ingest aborts");

    // Survey mode: the class survives with its other items; the gap is
    // recorded against the failing item only.
    survey::activate();
    let result = ingest_app_from_tree(tree());
    let gaps = survey::drain();
    let app = result.expect("survey-mode ingest recovers");
    let widget = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Widget")
        .expect("the model registers despite the unsupported item");
    assert!(
        widget.scopes().any(|s| s.name.as_str() == "fine"),
        "items after the unsupported one survive"
    );
    assert!(
        widget.associations().count() == 1,
        "items before the unsupported one survive"
    );
    assert!(
        gaps.iter().any(|g| matches!(
            g,
            IngestError::Unsupported { message, .. } if message.contains("scope :broken")
        )),
        "the failing item is recorded as a gap"
    );
}

#[test]
fn cattr_and_verbatim_classvar_reads_share_storage() {
    use roundhouse::expr::LValue;
    use roundhouse::ingest::ingest_library_classes;
    use roundhouse::{Expr, ExprNode};

    // extras/keybase.rb shape: cattr_accessor and verbatim `@@X` share
    // Rails class-variable storage; the `@@X = nil` seed is kept.
    let src = br#"class Keybase
  cattr_accessor :DOMAIN

  @@DOMAIN = nil

  def self.enabled?
    @@DOMAIN.present?
  end
end
"#;
    let classes = ingest_library_classes(src, "extras/keybase.rb").expect("ingest");
    let kb = &classes[0];

    fn has_classvar(e: &Expr, cvar: &str) -> bool {
        let mut found = false;
        fn walk(e: &Expr, cvar: &str, found: &mut bool) {
            if let ExprNode::Var { name, .. } = &*e.node {
                if name.as_str() == cvar {
                    *found = true;
                }
            }
            e.node.for_each_child(&mut |c| walk(c, cvar, found));
        }
        walk(e, cvar, &mut found);
        found
    }

    assert!(
        kb.class_ivar_initializers.iter().any(|expr| {
            matches!(
                &*expr.node,
                ExprNode::Assign {
                    target: LValue::Var { name, .. },
                    ..
                } if name.as_str() == "@@DOMAIN"
            )
        }),
        "@@DOMAIN = nil seed must survive"
    );
    let enabled = kb
        .methods
        .iter()
        .find(|m| m.name.as_str() == "enabled?")
        .expect("enabled? ingested");
    assert!(
        has_classvar(&enabled.body, "@@DOMAIN"),
        "class-method @@DOMAIN read must keep shared class-variable storage"
    );
    let reader = kb
        .methods
        .iter()
        .find(|m| m.name.as_str() == "DOMAIN")
        .expect("cattr reader synthesized");
    assert!(
        has_classvar(&reader.body, "@@DOMAIN"),
        "accessor reads @@DOMAIN"
    );
}

#[test]
fn non_nil_classvar_initializer_is_refused() {
    use roundhouse::ingest::ingest_library_classes;

    // A non-nil `@@X = <expr>` initializer can't be dropped silently —
    // strict ingest refuses it (survey mode records the gap per-item).
    let src = b"class Twitter\n  @@TIMEOUT = 30\nend\n";
    assert!(
        ingest_library_classes(src, "extras/twitter.rb").is_err(),
        "non-nil class-variable initializer must not be silently dropped"
    );
}

#[test]
fn exists_with_conditions_hash_desugars_to_where_chain() {
    // ActiveRecord `Model.exists?(code: x)` (and the `:code => x` form) is
    // the conditions overload — semantically `where(conditions).exists?`.
    // Ingest desugars it to that chain so the runtime keeps a monomorphic
    // `exists?`; the id form (`exists?(5)`) must stay untouched.
    use roundhouse::emit::ruby::emit_expr;

    fn ingest_first(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
    }

    let kwargs_form = emit_expr(&ingest_first(b"Invitation.exists?(code: self.code)"));
    assert!(
        kwargs_form.contains("where") && kwargs_form.ends_with(".exists?"),
        "kwargs conditions should chain through where(...).exists?; got: {kwargs_form}"
    );

    let rocket_form = emit_expr(&ingest_first(b"Invitation.exists?(:code => code)"));
    assert!(
        rocket_form.contains("where") && rocket_form.ends_with(".exists?"),
        "hash-rocket conditions should chain through where(...).exists?; got: {rocket_form}"
    );

    let relation_recv = emit_expr(&ingest_first(b"self.tags.exists?(tag: tag_name)"));
    assert!(
        relation_recv.contains("where") && relation_recv.ends_with(".exists?"),
        "relation-receiver conditions should chain too; got: {relation_recv}"
    );

    let id_form = emit_expr(&ingest_first(b"Invitation.exists?(5)"));
    assert!(
        !id_form.contains("where"),
        "id form must stay a plain exists?(id); got: {id_form}"
    );
}

#[test]
fn multi_write_with_attr_targets_ingests_and_round_trips() {
    // `self.a, self.b = pair` — the writer-method form of a multi-write.
    // Prism gives each target as a `CallTargetNode` named with the
    // setter's `=`; the IR carries the reader name in `LValue::Attr`,
    // and the Ruby emitter appends the assignment (#88).
    use roundhouse::emit::ruby::emit_expr;
    use roundhouse::expr::LValue;

    fn ingest_first(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
    }

    // Names and receiver kinds of a MultiAssign's targets, so the
    // comparison survives the span offsets moving between the source
    // and the emitted text.
    fn attr_targets(e: &Expr) -> Vec<(String, String)> {
        match *e.node {
            ExprNode::MultiAssign { ref targets, .. } => targets
                .iter()
                .map(|t| match t {
                    LValue::Attr { recv, name } => {
                        (emit_expr(recv), name.as_str().to_string())
                    }
                    other => panic!("expected an Attr target, got {other:?}"),
                })
                .collect(),
            ref other => panic!("expected a MultiAssign, got {other:?}"),
        }
    }

    // `self` receiver — the Sorbet `prop` writer shape from the report.
    let on_self = ingest_first(b"self.left, self.right = compute");
    assert_eq!(
        attr_targets(&on_self),
        vec![
            ("self".to_string(), "left".to_string()),
            ("self".to_string(), "right".to_string())
        ],
        "the trailing `=` belongs to the setter, not to the attribute name"
    );

    let emitted = emit_expr(&on_self);
    assert_eq!(
        attr_targets(&ingest_first(emitted.as_bytes())),
        attr_targets(&on_self),
        "emitted Ruby must re-ingest to the same targets; got:\n{emitted}"
    );

    // A receiver that is not `self`, and a mixed target list: the arm
    // reads the receiver as an ordinary expression, so anything that
    // ingests as one works.
    let on_other = ingest_first(b"pairing.left, pairing.right = compute");
    assert_eq!(
        attr_targets(&on_other),
        vec![
            ("pairing".to_string(), "left".to_string()),
            ("pairing".to_string(), "right".to_string())
        ]
    );
    assert_eq!(
        attr_targets(&ingest_first(emit_expr(&on_other).as_bytes())),
        attr_targets(&on_other)
    );

    let mixed = ingest_first(b"a, self.b = pair");
    match *mixed.node {
        ExprNode::MultiAssign { ref targets, .. } => {
            assert!(matches!(targets[0], LValue::Var { .. }));
            assert!(matches!(targets[1], LValue::Attr { ref name, .. } if name.as_str() == "b"));
        }
        ref other => panic!("expected a MultiAssign, got {other:?}"),
    }
}

/// `enum :x, CONST.map { |v| [v, v.to_s] }.to_h` — Procore's
/// `bid_package.rb` (`ACCOUNTING_METHODS.map { |method| [method,
/// method.to_s] }.to_h`) and `potential_change_order.rb` compute an
/// identity STRING mapping over a constant instead of writing the hash
/// out by hand. Unlike a bare `enum :x, CONST` (which stores each
/// label at its array INDEX, Rails' default), this form explicitly
/// stores each label as its own name — the resulting values must be
/// `Str`, not the `Int` a plain array mapping would give.
#[test]
fn computed_enum_map_to_h_over_constant_ingests() {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let computed_src = concat!(
        "class Widget < ApplicationRecord\n",
        "  ACCOUNTING_METHODS = %i[amount unit]\n",
        "  enum :accounting_method, ACCOUNTING_METHODS.map { |method| [method, method.to_s] }.to_h\n",
        "end\n",
    );

    let tree_for = |src: &str| -> HashMap<PathBuf, Vec<u8>> {
        [(PathBuf::from("app/models/widget.rb"), src.as_bytes().to_vec())].into_iter().collect()
    };

    let computed = ingest_app_from_tree(tree_for(computed_src))
        .expect("computed .map{}.to_h enum mapping ingests strict");

    let computed_widget = &computed.models[0];
    let column = roundhouse::Symbol::from("accounting_method");
    assert_eq!(
        computed_widget.enums.get(&column).unwrap(),
        &vec![
            ("amount".to_string(), Literal::Str { value: "amount".to_string() }),
            ("unit".to_string(), Literal::Str { value: "unit".to_string() }),
        ]
    );
}

/// `.index_by(&:to_s)` and `.index_with(&:to_s)` over a `CONST` — the
/// other two identity-string-mapping spellings alongside `.map{}.to_h`,
/// widened to accept a constant receiver (previously only a literal
/// `%w[…]` array).
#[test]
fn computed_enum_index_by_and_index_with_over_constant_ingest() {
    use roundhouse::ingest::ingest_app_from_tree;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let files: &[(&str, &str)] = &[(
        "app/models/widget.rb",
        concat!(
            "class Widget < ApplicationRecord\n",
            "  KINDS = %w[invisible nothing]\n",
            "  enum :kind, KINDS.index_by(&:to_s)\n",
            "  enum :variant, KINDS.index_with(&:to_s)\n",
            "end\n",
        ),
    )];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();

    let app = ingest_app_from_tree(tree)
        .expect("index_by/index_with over a constant ingest strict");
    let widget = &app.models[0];
    let expected = vec![
        ("invisible".to_string(), Literal::Str { value: "invisible".to_string() }),
        ("nothing".to_string(), Literal::Str { value: "nothing".to_string() }),
    ];
    assert_eq!(widget.enums.get(&roundhouse::Symbol::from("kind")).unwrap(), &expected);
    assert_eq!(widget.enums.get(&roundhouse::Symbol::from("variant")).unwrap(), &expected);
}

// ── Gap F15: block-argument forms other than `&:symbol`/`&local_var` ──

#[test]
fn block_arg_method_ref_bare() {
    fn parse_one(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    let e = parse_one(b"[1, 2].map(&method(:double))");
    let ExprNode::Send { block: Some(block), .. } = &*e.node else {
        panic!("expected Send, got {:?}", e.node);
    };
    match &*block.node {
        ExprNode::MethodRef { recv, name } => {
            assert!(recv.is_none(), "bare `method(:x)` has no receiver");
            assert_eq!(name.as_str(), "double");
        }
        other => panic!("expected MethodRef, got {other:?}"),
    }
}

#[test]
fn block_arg_method_ref_self_and_recv() {
    fn parse_one(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    let e = parse_one(b"[1, 2].map(&self.method(:triple))");
    let ExprNode::Send { block: Some(block), .. } = &*e.node else {
        panic!("expected Send, got {:?}", e.node);
    };
    match &*block.node {
        ExprNode::MethodRef { recv: Some(recv), name } => {
            assert!(matches!(&*recv.node, ExprNode::SelfRef));
            assert_eq!(name.as_str(), "triple");
        }
        other => panic!("expected MethodRef with SelfRef recv, got {other:?}"),
    }

    let e = parse_one(b"[1, 2].map(&widget.method(:quad))");
    let ExprNode::Send { block: Some(block), .. } = &*e.node else {
        panic!("expected Send, got {:?}", e.node);
    };
    match &*block.node {
        ExprNode::MethodRef { recv: Some(_), name } => {
            assert_eq!(name.as_str(), "quad");
        }
        other => panic!("expected MethodRef with a receiver, got {other:?}"),
    }
}

#[test]
fn block_arg_stabby_lambda_and_proc_desugar_to_lambda() {
    fn parse_one(source: &[u8]) -> Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    for src in [
        &b"[1, 2].each(&->(a) { a + 1 })"[..],
        &b"[1, 2].each(&proc { |a| a + 1 })"[..],
        &b"[1, 2].each(&lambda { |a| a + 1 })"[..],
    ] {
        let e = parse_one(src);
        let ExprNode::Send { block: Some(block), .. } = &*e.node else {
            panic!("expected Send, got {:?}", e.node);
        };
        match &*block.node {
            ExprNode::Lambda { params, .. } => {
                assert_eq!(params.len(), 1, "source: {}", String::from_utf8_lossy(src));
                assert_eq!(params[0].as_str(), "a");
            }
            other => panic!("expected Lambda, got {other:?} for {}", String::from_utf8_lossy(src)),
        }
    }
}

#[test]
fn block_arg_ivar_and_call_result_preserve_the_forwarded_expression() {
    for (source, is_ivar) in [(b"[1, 2].each(&@callback)".as_slice(), true), (b"[1, 2].each(&compute(1))".as_slice(), false)] {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
        let expr = roundhouse::ingest::ingest_expr(&stmt, "<literal>").expect("block operand ingests");
        let ExprNode::Send { block: Some(block), .. } = &*expr.node else { panic!("missing block operand") };
        if is_ivar {
            assert!(matches!(&*block.node, ExprNode::Ivar { name } if name.as_str() == "callback"));
        } else {
            assert!(matches!(&*block.node, ExprNode::Send { method, args, .. } if method.as_str() == "compute" && args.len() == 1));
        }
    }
}

#[test]
fn defined_extended_targets_ingest_and_round_trip() {
    // Gap #18.2: retain Tim Tischler's constant, call and super controls.
    use roundhouse::emit::ruby::emit_expr;

    for source in ["defined?(Widget)", "defined?(Widget::Kind)", "defined?(widget.kind)", "defined?(super)"] {
        let parse = |source: &str| {
            let result = ruby_prism::parse(source.as_bytes());
            let program = result.node();
            let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
            roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
        };
        let expr = parse(source);
        let ExprNode::Defined { operand } = &*expr.node else {
            panic!("expected native defined? syntax: {expr:?}");
        };
        match source {
            "defined?(Widget)" => assert!(matches!(&*operand.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["Widget"])),
            "defined?(Widget::Kind)" => assert!(matches!(&*operand.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["Widget", "Kind"])),
            "defined?(widget.kind)" => assert!(matches!(&*operand.node, ExprNode::Send { recv: Some(_), method, .. } if method.as_str() == "kind")),
            _ => assert!(matches!(&*operand.node, ExprNode::Super { args: None })),
        }
        let emitted = emit_expr(&expr);
        assert_eq!(emit_expr(&parse(&emitted)), emitted);
        let mut children = 0;
        expr.node.for_each_child(&mut |_| children += 1);
        assert_eq!(children, 0, "a syntax query must not expose value children");
    }
}

#[test]
fn class_variable_compound_assignment_in_method_body_ingests_and_round_trips() {
    use roundhouse::emit::ruby::emit_expr;
    use roundhouse::expr::{LValue, OpAssignOp};

    for source in ["@@count ||= 0", "@@count = 1"] {
        let parse = |source: &str| {
            let result = ruby_prism::parse(source.as_bytes());
            let program = result.node();
            let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
            roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap()
        };
        let expr = parse(source);
        if source.contains("||=") {
            assert!(matches!(&*expr.node, ExprNode::OpAssign { target: LValue::Var { name, .. }, op: OpAssignOp::OrOr, .. } if name.as_str() == "@@count"));
        } else {
            assert!(matches!(&*expr.node, ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name.as_str() == "@@count"));
        }
        assert_eq!(emit_expr(&expr), source);
        assert_eq!(emit_expr(&parse(source)), source);
    }
}

#[test]
fn specific_ledger_messages_replace_the_generic_catch_all() {
    use roundhouse::ingest::IngestError;

    for (source, expected) in [
        ("`ls`", "shell command (backticks) is not modeled"),
        ("%x{ls}", "shell command (backticks) is not modeled"),
        ("$stdout = out", "global variable write"),
        ("class Foo; end", "class/module defined inside a method or block (runtime class definition)"),
        ("module Foo; end", "class/module defined inside a method or block (runtime class definition)"),
        ("1 + ", "unparsed fragment (Prism recovery node)"),
    ] {
        let result = ruby_prism::parse(source.as_bytes());
        let program = result.node();
        let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
        let Err(IngestError::Unsupported { message, .. }) = roundhouse::ingest::ingest_expr(&stmt, "<snippet>") else {
            panic!("expected unsupported: {source}");
        };
        assert_eq!(message, expected);
    }
}

#[test]
fn multi_write_with_post_rest_targets_ingests_and_round_trips() {
    use roundhouse::emit::ruby::emit_expr;

    let parse = |source: &str| {
        let result = ruby_prism::parse(source.as_bytes());
        let program = result.node();
        roundhouse::ingest::ingest_expr(&program.as_program_node().unwrap().statements().as_node(), "<snippet>").unwrap()
    };
    let expr = parse("a, *b, c = [1, 2, 3, 4]");
    let emitted = emit_expr(&expr);
    assert!(emitted.contains("a = "), "{emitted}");
    assert!(emitted.contains(".drop(1).take("), "{emitted}");
    assert!(emitted.contains("[-1]"), "{emitted}");
    assert_eq!(expr, parse(&emitted), "round-trip IR, not only emitted text, must be stable");
    assert_eq!(emit_expr(&parse(&emitted)), emitted);
}

#[test]
fn multi_write_temporary_does_not_capture_a_user_target() {
    let source = "a, *__mw_0, c = [11, 22, 33]";
    let result = ruby_prism::parse(source.as_bytes());
    let stmt = result.node().as_program_node().unwrap().statements().body().iter().next().unwrap();
    let expr = roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap();
    let ExprNode::Seq { exprs } = &*expr.node else { panic!("expected desugared assignment") };
    let ExprNode::Assign { target: LValue::Var { name, .. }, .. } = &*exprs[0].node else {
        panic!("expected temporary binding");
    };
    assert_ne!(name.as_str(), "__mw_0");
}

#[test]
fn simple_defined_operands_keep_ruby_descriptors() {
    for (source, expected) in [
        ("defined?(self)", "self"), ("defined?(nil)", "nil"),
        ("defined?(true)", "true"), ("defined?(false)", "false"),
        ("defined?(17)", "expression"),
    ] {
        let result = ruby_prism::parse(source.as_bytes());
        let stmt = result.node().as_program_node().unwrap().statements().body().iter().next().unwrap();
        let expr = roundhouse::ingest::ingest_expr(&stmt, "<snippet>").unwrap();
        assert!(matches!(&*expr.node, ExprNode::Lit { value: Literal::Str { value } } if value == expected), "{source}: {expr:?}");
    }
}

#[test]
fn post_rest_effectful_targets_remain_explicitly_unsupported() {
    for source in [
        "a, *, mark(log)[0] = [rhs(log)]",
        "mark(log)[0], *b, c = [rhs(log)]",
        "a, *mark(log)[0], c = [rhs(log)]",
        "a, *, target.value = [rhs(log)]",
    ] {
        let result = ruby_prism::parse(source.as_bytes());
        assert_eq!(result.errors().count(), 0, "legal Ruby control: {source}");
        let program = result.node();
        let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
        let err = roundhouse::ingest::ingest_expr(&stmt, "<snippet>").expect_err("LHS order must not change silently");
        assert!(err.to_string().contains("preserved LHS evaluation order"), "{err}");
    }
}

#[test]
fn class_method_classvar_writes_keep_shared_inheritance_storage() {
    use roundhouse::expr::ExprNode;
    for method in ["def self.bump", "class << self; def bump"] {
        for write in ["@@count ||= 11", "@@count = 14", "@@count &&= 17", "@@count += 3", "@@count -= 1"] {
            let extra_end = if method.starts_with("class") { "end" } else { "" };
            let source = format!("class Parent; {method}; {write}; @@count; end; {extra_end}; end\nclass Child < Parent; end");
            let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
                .expect("shared @@ storage must ingest on class methods");
            let bump = classes[0]
                .methods
                .iter()
                .find(|m| m.name.as_str() == "bump")
                .expect("bump");
            let mut saw = false;
            bump.body.node.for_each_child(&mut |c| {
                if matches!(&*c.node, ExprNode::Var { name, .. } if name.as_str() == "@@count") {
                    saw = true;
                }
            });
            // OpAssign / Assign targets are LValues, not child Exprs — also
            // accept bodies whose emitted text would read @@count.
            let text = format!("{:?}", bump.body.node);
            assert!(
                saw || text.contains("@@count"),
                "expected @@count in bump body: {text}"
            );
        }
    }
}

#[test]
fn post_rest_nonliteral_rhs_remains_unsupported_without_coercion() {
    for rhs in ["11", "nil", "Coercible.new", "values", "[11, 22].dup"] {
        let source = format!("a, *b, c = {rhs}");
        let result = ruby_prism::parse(source.as_bytes());
        assert_eq!(result.errors().count(), 0, "legal Ruby control: {source}");
        let program = result.node();
        let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
        let err = roundhouse::ingest::ingest_expr(&stmt, "<snippet>")
            .expect_err("collection methods do not implement Ruby coercion");
        assert!(err.to_string().contains("to_ary coercion"), "{err}");
    }
}

#[test]
fn richer_defined_call_shapes_remain_explicitly_unsupported() {
    for source in ["defined?(self.call(11))", "defined?(self.call {})", "defined?(self&.call)"] {
        let result = ruby_prism::parse(source.as_bytes());
        assert_eq!(result.errors().count(), 0);
        let program = result.node();
        let stmt = program.as_program_node().unwrap().statements().body().iter().next().unwrap();
        let err = roundhouse::ingest::ingest_expr(&stmt, "<snippet>").expect_err("unverified query shape");
        assert!(err.to_string().contains("defined? calls"), "{err}");
    }
}

#[test]
fn mattr_and_native_classvar_writes_share_storage() {
    // Rails mattr/cattr is @@; a hand-written @@ write is the same slot.
    for declaration in ["cattr_accessor", "mattr_accessor"] {
        let source = format!(
            "class Probe; {declaration} :count; def bump; @@count = 11; end; def self.current; @@count; end; end"
        );
        let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
            .expect("shared @@ storage must ingest");
        assert!(
            classes[0].class_ivar_initializers.iter().any(|expr| {
                matches!(
                    &*expr.node,
                    ExprNode::Assign {
                        target: LValue::Var { name, .. },
                        ..
                    } if name.as_str() == "@@count"
                ) || matches!(
                    &*expr.node,
                    ExprNode::If { else_branch, .. } if matches!(
                        &*else_branch.node,
                        ExprNode::Assign {
                            target: LValue::Var { name, .. },
                            ..
                        } if name.as_str() == "@@count"
                    )
                )
            }),
            "mattr seeds @@count = nil: {:?}",
            classes[0].class_ivar_initializers
        );
    }
}

#[test]
fn cattr_defaults_cannot_be_silently_dropped_with_native_initializers() {
    for declaration in ["cattr_reader", "cattr_writer", "cattr_accessor", "mattr_reader", "mattr_writer", "mattr_accessor"] {
        for default in ["default: 41", "default: nil", ""] {
            let call = if default.is_empty() {
                format!("{declaration}(:count) {{ 41 }}")
            } else {
                format!("{declaration} :count, {default}")
            };
            for body in [format!("@@count = nil; {call}"), format!("{call}; @@count = nil")] {
                let source = format!("class Probe; {body}; def self.current; @@count; end; end");
                let err = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
                    .expect_err("an explicit default must not share a body with a native @@ seed");
                assert!(err.to_string().contains("cattr/mattr defaults require source-order initialization"), "{err}");
            }
            // Standalone parseable defaults seed @@attr = <value>.
            let source = format!("class Probe; {call}; end");
            let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
                .expect("standalone class-attribute default must seed");
            let seed_value = classes[0].class_ivar_initializers.iter().find_map(|expr| {
                match &*expr.node {
                    ExprNode::Assign {
                        target: LValue::Var { name, .. },
                        value,
                    } if name.as_str() == "@@count" => Some(value),
                    // Nil defaults are conditional (`unless class_variable_defined?`).
                    ExprNode::If { else_branch, .. } => match &*else_branch.node {
                        ExprNode::Assign {
                            target: LValue::Var { name, .. },
                            value,
                        } if name.as_str() == "@@count" => Some(value),
                        _ => None,
                    },
                    _ => None,
                }
            });
            assert!(
                seed_value.is_some(),
                "missing @@count seed for {call}: {:?}",
                classes[0].class_ivar_initializers
            );
            let value = seed_value.unwrap();
            let expected_nil = default == "default: nil";
            assert_eq!(
                matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }),
                expected_nil,
                "{call}: {:?}",
                value.node
            );
            if default == "default: 41" || default.is_empty() {
                assert!(
                    matches!(&*value.node, ExprNode::Lit { value: Literal::Int { value: 41 } }),
                    "{call}: {:?}",
                    value.node
                );
            }
        }
        for default in ["**{default: 41}", "**options", "instance_reader: false, default: 41"] {
            let call = format!("{declaration} :count, {default}");
            let source = format!("class Probe; {call}; end");
            let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
                .expect("unmodeled defaults leave the class standing");
            assert!(
                classes[0].class_ivar_initializers.iter().all(|expr| {
                    !matches!(
                        &*expr.node,
                        ExprNode::Assign {
                            target: LValue::Var { name, .. },
                            ..
                        } if name.as_str() == "@@count"
                    )
                }),
                "unmodeled default must not nil-seed @@count: {:?}",
                classes[0].class_ivar_initializers
            );
            assert!(
                !classes[0].methods.iter().any(|m| m.name.as_str() == "count"),
                "unmodeled default must not synthesize accessors"
            );
            assert!(
                !classes[0].unknown_calls.is_empty(),
                "unmodeled mattr/cattr must remain an unknown call"
            );
        }
        let source = format!("class Probe; @@count = nil; {declaration} :count; end");
        let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb").unwrap();
        assert_eq!(
            classes[0].class_ivar_initializers.len(),
            1,
            "source @@nil seed is kept for shared mattr storage"
        );
        assert!(matches!(
            &*classes[0].class_ivar_initializers[0].node,
            ExprNode::Assign {
                target: LValue::Var { name, .. },
                ..
            } if name.as_str() == "@@count"
        ));
    }
}

#[test]
fn native_classvar_initialization_uses_owned_initializer_ir() {
    let classes = roundhouse::ingest::ingest_library_classes(
        b"class Probe; @@count = nil; def self.current; @@count; end; end", "probe.rb",
    ).unwrap();
    assert!(classes[0].unknown_calls.is_empty());
    assert!(matches!(&*classes[0].class_ivar_initializers[0].node,
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name.as_str() == "@@count"));
    for declaration in ["arbitrary_dsl", "INITIAL = @@count", "include Other"] {
        let source = format!("class Probe; @@count = nil; {declaration}; end");
        let err = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
            .expect_err("separate class-body buckets cannot preserve interleaving");
        assert!(err.to_string().contains("requires source ordering"), "{err}");
    }
}

#[test]
fn native_initializers_keep_order_across_singleton_body_merges() {
    let classes = roundhouse::ingest::ingest_library_classes(
        b"class Probe; @@before=nil; class << self; @@middle=nil; end; @@after=nil; end",
        "recursive_initializer.rb",
    ).unwrap();
    let names: Vec<_> = classes[0].class_ivar_initializers.iter().map(|expr| {
        assert!(!expr.span.is_synthetic());
        match &*expr.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, .. } => name.as_str(),
            other => panic!("unexpected initializer: {other:?}"),
        }
    }).collect();
    assert_eq!(names, ["@@before", "@@middle", "@@after"]);
}

#[test]
fn case_in_pattern_matching() {
    use roundhouse::expr::{HashRest, MatchGuardKind, MatchPattern};

    fn parse_one(source: &[u8]) -> roundhouse::expr::Expr {
        let result = ruby_prism::parse(source);
        let program = result.node();
        let prog = program.as_program_node().unwrap();
        let stmt = prog.statements().body().iter().next().unwrap();
        roundhouse::ingest::ingest_expr(&stmt, "<literal>").unwrap()
    }

    // `nil`, a bare bind, and a guard (`if`).
    let e = parse_one(
        b"case p\nin nil\n  0\nin company if company.present?\n  1\nend",
    );
    let ExprNode::CaseMatch { arms, else_body, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    assert!(else_body.is_none());
    assert_eq!(arms.len(), 2);
    assert!(matches!(arms[0].pattern, MatchPattern::Nil));
    assert!(arms[0].guard.is_none());
    match &arms[1].pattern {
        MatchPattern::Bind { name } => assert_eq!(name.as_str(), "company"),
        other => panic!("expected Bind, got {other:?}"),
    }
    match &arms[1].guard {
        Some((MatchGuardKind::If, _)) => {}
        other => panic!("expected an `if` guard, got {other:?}"),
    }

    // Constant-narrowed array-style deconstruct (`Success(page)`) — the
    // dominant real-world shape (dry-monads `Result`).
    let e = parse_one(b"case r\nin Success(page)\n  page\nin Failure(error)\n  error\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    match &arms[0].pattern {
        MatchPattern::Array { constant: Some(c), pre, rest, post } => {
            assert!(matches!(&*c.node, ExprNode::Const { .. }));
            assert_eq!(pre.len(), 1);
            assert!(rest.is_none());
            assert!(post.is_empty());
            match &pre[0] {
                MatchPattern::Bind { name } => assert_eq!(name.as_str(), "page"),
                other => panic!("expected Bind, got {other:?}"),
            }
        }
        other => panic!("expected constant-narrowed Array, got {other:?}"),
    }

    // Array pattern with a leading Capture and a named rest.
    let e = parse_one(b"case a\nin [Integer => n, *rest]\n  n\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    match &arms[0].pattern {
        MatchPattern::Array { constant: None, pre, rest: Some(Some(rest_name)), post } => {
            assert_eq!(rest_name.as_str(), "rest");
            assert!(post.is_empty());
            match &pre[0] {
                MatchPattern::Capture { pattern, name } => {
                    assert_eq!(name.as_str(), "n");
                    assert!(matches!(&**pattern, MatchPattern::Value { .. }));
                }
                other => panic!("expected Capture, got {other:?}"),
            }
        }
        other => panic!("expected plain Array with named rest, got {other:?}"),
    }

    // Find pattern: bare splats on both sides around a fixed middle.
    let e = parse_one(b"case a\nin [*, 5, *post]\n  post\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    match &arms[0].pattern {
        MatchPattern::Find { constant: None, pre_rest: None, middle, post_rest: Some(name) } => {
            assert_eq!(middle.len(), 1);
            assert_eq!(name.as_str(), "post");
        }
        other => panic!("expected Find, got {other:?}"),
    }

    // Hash pattern: value-omission bind, explicit sub-pattern, and
    // `**rest` collection.
    let e = parse_one(b"case h\nin {status: \"ok\", data:, **rest}\n  data\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    match &arms[0].pattern {
        MatchPattern::Hash { constant: None, pairs, rest: Some(HashRest::Collect { name }) } => {
            assert_eq!(name.as_str(), "rest");
            assert_eq!(pairs.len(), 2);
            assert_eq!(pairs[0].0.as_str(), "status");
            assert!(pairs[0].1.is_some());
            assert_eq!(pairs[1].0.as_str(), "data");
            assert!(pairs[1].1.is_none());
        }
        other => panic!("expected Hash with **rest, got {other:?}"),
    }

    // `**nil` — no unmatched keys allowed.
    let e = parse_one(b"case h\nin {a:, **nil}\n  a\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    assert!(matches!(
        &arms[0].pattern,
        MatchPattern::Hash { rest: Some(HashRest::Nil), .. }
    ));

    // Alternation flattens Prism's binary tree to one flat Vec.
    let e = parse_one(b"case x\nin 'a' | 'b' | 'c'\n  1\nend");
    let ExprNode::CaseMatch { arms, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    match &arms[0].pattern {
        MatchPattern::Alt { alternatives } => assert_eq!(alternatives.len(), 3),
        other => panic!("expected Alt, got {other:?}"),
    }

    // Pin operator: `^name` and `^(expr)` both fold into `Value`.
    let e = parse_one(b"case x\nin ^expected\n  1\nelse\n  2\nend");
    let ExprNode::CaseMatch { arms, else_body, .. } = &*e.node else {
        panic!("expected CaseMatch, got {:?}", e.node);
    };
    assert!(else_body.is_some());
    match &arms[0].pattern {
        MatchPattern::Value { expr } => assert!(matches!(&*expr.node, ExprNode::Var { .. })),
        other => panic!("expected pinned Value, got {other:?}"),
    }

    // `value in pattern` — MatchPredicate, never raises.
    let e = parse_one(b"x in Integer");
    match &*e.node {
        ExprNode::MatchPredicate { pattern, .. } => {
            assert!(matches!(pattern, MatchPattern::Value { .. }));
        }
        other => panic!("expected MatchPredicate, got {other:?}"),
    }

    // `value => pattern` — MatchRequired, binds or raises.
    let e = parse_one(b"y => Integer");
    assert!(matches!(&*e.node, ExprNode::MatchRequired { .. }));
}

#[test]
fn nested_class_methods_cannot_relocate_native_initializers() {
    use roundhouse::ingest::ingest_library_classes;
    let err = ingest_library_classes(
        b"module Probe; module ClassMethods; @@flag = nil; def flag; @@flag; end; end; end",
        "probe.rb",
    ).expect_err("ClassMethods owns @@flag, not the enclosing Probe");
    assert!(err.to_string().contains("class-variable initialization in module ClassMethods is not modeled"), "{err}");

    // A cattr in the same body shares @@ storage; the nil seed relocates
    // onto Probe with the ClassMethods methods.
    let classes = ingest_library_classes(
        b"module Probe; module ClassMethods; @@flag = nil; cattr_accessor :flag; def read; @@flag; end; end; end",
        "probe.rb",
    ).unwrap();
    let probe = classes.iter().find(|class| class.name.0.as_str() == "Probe").unwrap();
    assert!(
        probe.class_ivar_initializers.iter().any(|expr| {
            matches!(
                &*expr.node,
                roundhouse::expr::ExprNode::Assign {
                    target: roundhouse::expr::LValue::Var { name, .. },
                    ..
                } if name.as_str() == "@@flag"
            )
        }),
        "@@flag seed must fold onto Probe: {:?}",
        probe.class_ivar_initializers
    );
    assert!(probe.methods.iter().any(|method| method.name.as_str() == "read"));

    // An initializer actually owned by Probe must not be rejected.
    let classes = ingest_library_classes(
        b"module Probe; @@flag = nil; module ClassMethods; def flag; @@flag; end; end; end",
        "probe.rb",
    ).unwrap();
    let probe = classes.iter().find(|class| class.name.0.as_str() == "Probe").unwrap();
    assert_eq!(probe.class_ivar_initializers.len(), 1);

    // Plain ClassMethods cattr (no source @@) still relocates a nil seed.
    let classes = ingest_library_classes(
        b"module Probe; module ClassMethods; cattr_accessor :flag; end; end",
        "probe.rb",
    ).unwrap();
    let probe = classes.iter().find(|class| class.name.0.as_str() == "Probe").unwrap();
    assert!(
        probe.class_ivar_initializers.iter().any(|expr| {
            matches!(
                &*expr.node,
                roundhouse::expr::ExprNode::Assign {
                    target: roundhouse::expr::LValue::Var { name, .. },
                    ..
                } if name.as_str() == "@@flag"
            ) || matches!(
                &*expr.node,
                roundhouse::expr::ExprNode::If { else_branch, .. } if matches!(
                    &*else_branch.node,
                    roundhouse::expr::ExprNode::Assign {
                        target: roundhouse::expr::LValue::Var { name, .. },
                        ..
                    } if name.as_str() == "@@flag"
                )
            )
        }),
        "ClassMethods cattr must seed @@flag on Probe: {:?}",
        probe.class_ivar_initializers
    );
    assert!(probe.methods.iter().any(|m| m.name.as_str() == "flag"));
}

#[test]
fn direct_class_and_module_compound_ivar_writes_are_initializers() {
    use roundhouse::expr::OpAssignOp;
    use roundhouse::ingest::ingest_library_classes;

    for kind in ["class", "module"] {
        let source = format!("{kind} Probe\n  @cache ||= 7\n  @cache &&= 8\n  @cache += 1\n  \
            class << self\n    @singleton_cache ||= 9\n  end\n  def self.cache; @cache; end\nend\n");
        let classes = ingest_library_classes(source.as_bytes(), "probe.rb").unwrap();
        let probe = classes.iter().find(|class| class.name.0.as_str() == "Probe").unwrap();
        assert!(probe.unknown_calls.is_empty(), "compound writes belong to the initializer list");
        assert_eq!(probe.class_ivar_initializers.len(), 3, "{kind} body loses a compound write");
        for (initializer, expected_op) in probe.class_ivar_initializers.iter()
            .zip([OpAssignOp::OrOr, OpAssignOp::AndAnd, OpAssignOp::Add])
        {
            assert!(matches!(&*initializer.node,
                ExprNode::OpAssign { target: LValue::Ivar { name }, op, .. }
                    if name.as_str() == "cache" && *op == expected_op));
            assert_eq!(&source[initializer.span.start as usize..initializer.span.end as usize],
                match expected_op {
                    OpAssignOp::OrOr => "@cache ||= 7",
                    OpAssignOp::AndAnd => "@cache &&= 8",
                    OpAssignOp::Add => "@cache += 1",
                    _ => unreachable!(),
                });
        }
    }
}

/// Parameters after a rest (`->(*, payload)`, `|*rest, a, b|`) used to
/// vanish: Lambda IR has no slot for them, so `->(*, payload) {
/// payload[:sql] }` emitted as `-> { payload[:sql] }`, a body reading a
/// name nothing bound, and the expression IR diverged across a round
/// trip. They are now popped off the rest, last first, which is Ruby's
/// own rule, and the emitted form reaches a fixed point.
#[test]
fn parameters_after_a_rest_are_popped_off_it() {
    use roundhouse::emit::ruby::emit_expr;
    let cases: &[(&[u8], &[&str])] = &[
        (b"cb = ->(*, payload) { payload[:sql] }", &["->(*__rest)", "payload = __rest.pop"]),
        (b"cb = ->(*rest, a, b) { [rest, a, b] }", &["->(*rest)", "b = rest.pop", "a = rest.pop"]),
        (b"cb = ->(*args) { args }", &["->(*args)"]),
        (b"xs.each { |*, last| p last }", &["|*__rest|", "last = __rest.pop"]),
    ];
    for (source, wants) in cases {
        let first = emit_expr(&ingest_snippet(source));
        for want in *wants {
            assert!(first.contains(want), "{}: expected `{want}` in:\n{first}", String::from_utf8_lossy(source));
        }
        let second = emit_expr(&ingest_snippet(first.as_bytes()));
        assert_eq!(first, second, "{} is not a fixed point", String::from_utf8_lossy(source));
    }
}
