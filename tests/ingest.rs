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
        Association::HasMany { name, target, foreign_key, through, dependent, scope } => {
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
        Association::BelongsTo { name, target, foreign_key, optional } => {
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
    assert_eq!(cb.target.as_str(), "normalize_title");
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
    // One unknown DSL entry (`devise_for`) must not zero the table:
    // survey mode records the gap and keeps the sibling routes;
    // strict mode still fails loud so fixtures force recognizers.
    let source = br#"Rails.application.routes.draw do
  devise_for :users
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
        gaps.iter().any(|g| format!("{g:?}").contains("devise_for")),
        "the devise_for gap is recorded, not silently dropped: {gaps:?}"
    );
}

#[test]
fn routes_mount_drops_as_recognized_gap() {
    // `mount SomeEngine` is external code, never part of the
    // transpiled app: strict ingest drops the route (the modeled
    // truth, like `to: redirect(...)`), survey runs get a ledger
    // line so the drop stays visible.
    let source = br#"Rails.application.routes.draw do
  mount Sidekiq::Web, at: "sidekiq"
  get "/posts", to: "posts#index"
end
"#;

    let (strict, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let table = strict.expect("strict ingest tolerates mount");
    assert_eq!(table.entries.len(), 1, "mount drops, the sibling route survives");

    roundhouse::ingest::survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_routes(source, "config/routes.rb")
    });
    let gaps = roundhouse::ingest::survey::drain();
    result.expect("survey ingest succeeds");
    assert!(
        gaps.iter().any(|g| format!("{g:?}").contains("mount")),
        "the mount drop is ledgered, not silent: {gaps:?}"
    );
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
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
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
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
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
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
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
        |r| matches!(r, ValidationRule::Length { min: Some(10), max: None })
    ));

    let title_v = validations.iter().find(|v| v.attribute.as_str() == "title").unwrap();
    assert_eq!(title_v.rules.len(), 1);
    assert!(matches!(
        title_v.rules[0],
        ValidationRule::Length { min: None, max: Some(80) }
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
    let model = roundhouse::ingest::ingest_model(source, "<inline>", &schema)
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
/// and the surfacing that keeps unsupported (Slim/`.text.erb`/`.ruby`)
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
        // A Slim view: still an unsupported engine the analyzer skips.
        ("app/views/widgets/show.html.slim", "h1 = @widget.name\n"),
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
            .any(|m| m.contains("view template not ingested: slim")),
        "skipped Slim view should be recorded as a gap, got: {messages:?}"
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
fn cattr_classvar_bodies_normalize_to_class_ivars() {
    use roundhouse::ingest::ingest_library_classes;
    use roundhouse::{Expr, ExprNode};

    // The extras/keybase.rb shape: cattr_accessor storage and verbatim
    // `@@X` reads must agree (class-level ivar), and the `@@X = nil`
    // body initializer drops as semantically exact.
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

    fn has_classvar(e: &Expr) -> bool {
        let mut found = false;
        fn walk(e: &Expr, found: &mut bool) {
            if let ExprNode::Var { name, .. } = &*e.node {
                if name.as_str().starts_with("@@") {
                    *found = true;
                }
            }
            e.node.for_each_child(&mut |c| walk(c, found));
        }
        walk(e, &mut found);
        found
    }
    fn reads_ivar(e: &Expr, ivar: &str) -> bool {
        let mut found = false;
        fn walk(e: &Expr, ivar: &str, found: &mut bool) {
            if let ExprNode::Ivar { name } = &*e.node {
                if name.as_str() == ivar {
                    *found = true;
                }
            }
            e.node.for_each_child(&mut |c| walk(c, ivar, found));
        }
        walk(e, ivar, &mut found);
        found
    }

    let enabled = kb
        .methods
        .iter()
        .find(|m| m.name.as_str() == "enabled?")
        .expect("enabled? ingested");
    assert!(
        !has_classvar(&enabled.body) && reads_ivar(&enabled.body, "DOMAIN"),
        "class-method @@DOMAIN read should normalize to the @DOMAIN class ivar"
    );
    // The cattr_accessor reader uses the same storage.
    let reader = kb
        .methods
        .iter()
        .find(|m| m.name.as_str() == "DOMAIN")
        .expect("cattr reader synthesized");
    assert!(reads_ivar(&reader.body, "DOMAIN"), "accessor reads @DOMAIN");
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
