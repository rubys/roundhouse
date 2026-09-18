//! Route DSL shapes Rails normalizes and the ingester used to drop or
//! mis-lower (#82, #84, #85):
//!
//! - `root to: redirect("/scan")` came back as a Root with an EMPTY
//!   target, which the flattener turned into `Route.new("GET", "/",
//!   :, :index)` — the one emitted file that did not parse, and the
//!   entry point. It is now dropped with a survey line, like `mount`.
//! - `resources :tasks, param: :task_id` bound `/tasks/:id` while the
//!   lowered controller read `params[:task_id]`.
//! - `resources "tours"` (String name) dropped the whole resource, and
//!   `only: %w[index show]` parsed to an EMPTY list, which the expander
//!   reads as "all seven actions" — so a two-action resource became
//!   seven routes, five to actions the controller does not define.

use roundhouse::App;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::flatten_routes;

fn routes(routes_rb: &str) -> Vec<(String, String, Vec<String>)> {
    let table = ingest_routes(routes_rb.as_bytes(), "config/routes.rb").expect("ingest routes");
    let mut app = App::default();
    app.routes = table;
    flatten_routes(&app)
        .into_iter()
        .map(|r| (format!("{:?}", r.method), r.path.clone(), r.path_params.clone()))
        .collect()
}

fn paths(routes_rb: &str) -> Vec<String> {
    routes(routes_rb).into_iter().map(|(m, p, _)| format!("{m} {p}")).collect()
}

#[test]
fn root_redirect_is_dropped_not_emitted_with_an_empty_controller() {
    let table = ingest_routes(
        br#"Rails.application.routes.draw do
  root to: redirect("/scan")
  get "/scan", to: "scans#index"
end
"#,
        "config/routes.rb",
    )
    .expect("ingest routes");
    // No Root entry at all — an empty-target Root is what became `:`.
    assert!(
        !table.entries.iter().any(|e| matches!(e, roundhouse::dialect::RouteSpec::Root { .. })),
        "root redirect must not produce a Root entry: {:?}",
        table.entries
    );
    let mut app = App::default();
    app.routes = table;
    let flat = flatten_routes(&app);
    assert_eq!(flat.len(), 1, "{flat:?}");
    assert_eq!(flat[0].path, "/scan");
}

#[test]
fn root_redirect_records_a_survey_line() {
    roundhouse::ingest::survey::activate();
    ingest_routes(
        br#"Rails.application.routes.draw do
  root to: redirect("/scan")
end
"#,
        "config/routes.rb",
    )
    .expect("ingest routes");
    let gaps = roundhouse::ingest::survey::drain();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].to_string().contains("`root` with a non-string target"), "{}", gaps[0]);
}

#[test]
fn resources_param_renames_the_member_segment() {
    let got = routes(
        r#"Rails.application.routes.draw do
  namespace :admin do
    resources :tasks, only: %i[index show], param: :task_id
  end
end
"#,
    );
    assert_eq!(
        got,
        vec![
            ("Get".to_string(), "/admin/tasks".to_string(), vec![]),
            ("Get".to_string(), "/admin/tasks/:task_id".to_string(), vec!["task_id".to_string()]),
        ]
    );
}

#[test]
fn resources_param_flows_into_nested_children_and_member_blocks() {
    // Rails: a child of `resources :tasks, param: :task_id` nests
    // under `:task_task_id`; a `member do` route binds `:task_id`.
    let got = paths(
        r#"Rails.application.routes.draw do
  resources :tasks, only: [:show], param: :task_id do
    resources :notes, only: [:index]
    member do
      post "archive"
    end
  end
end
"#,
    );
    assert_eq!(
        got,
        vec![
            "Get /tasks/:task_id",
            "Get /tasks/:task_task_id/notes",
            "Post /tasks/:task_id/archive",
        ]
    );
}

#[test]
fn string_resource_name_is_accepted() {
    let got = paths(
        r#"Rails.application.routes.draw do
  resources "tours", only: %i[index new create edit update]
end
"#,
    );
    assert_eq!(
        got,
        vec![
            "Get /tours",
            "Get /tours/new",
            "Post /tours",
            "Get /tours/:id/edit",
            "Patch /tours/:id",
            "Put /tours/:id",
        ]
    );
}

#[test]
fn string_only_list_restricts_like_the_symbol_form() {
    let symbols = paths(
        r#"Rails.application.routes.draw do
  resources :tasks, only: %i[index show]
end
"#,
    );
    let strings = paths(
        r#"Rails.application.routes.draw do
  resources :tasks, only: %w[index show]
end
"#,
    );
    assert_eq!(symbols, vec!["Get /tasks", "Get /tasks/:id"]);
    assert_eq!(strings, symbols);
}

#[test]
fn non_literal_only_is_an_error_not_a_widening() {
    // `only: ACTIONS` cannot be read; treating it as "no restriction"
    // would emit routes the app never declared.
    let err = ingest_routes(
        br#"Rails.application.routes.draw do
  resources :tasks, only: READ_ONLY
end
"#,
        "config/routes.rb",
    )
    .expect_err("a non-literal only: must not parse as empty");
    assert!(err.to_string().contains("`only:` is not a literal list"), "{err}");
}
