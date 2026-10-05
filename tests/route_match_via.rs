//! `match "path", to: "c#a", via: …` registers the route for the verbs
//! `via:` lists.
//!
//! Measured against Rails 8.1 (`bin/rails routes`):
//!
//! ```text
//! GET|POST /widget_lookup/:id  widgets#show
//!          /widget_any/:id     widgets#show     (via: :all, every verb)
//! ```
//!
//! The route used to lower as a single `"ANY"` row whatever `via:` said,
//! and the runtime router compared that verb literally against the
//! request's, so no request ever reached it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::App;
use roundhouse::dialect::HttpMethod;
use roundhouse::ingest::ingest_routes;
use roundhouse::lower::routes::{FlatRoute, flatten_routes};

fn routes(source: &str) -> Vec<FlatRoute> {
    let mut app = App::default();
    app.routes = ingest_routes(
        format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(),
        "config/routes.rb",
    )
    .expect("ingest routes");
    flatten_routes(&app)
}

fn verbs(routes: &[FlatRoute], path: &str) -> Vec<HttpMethod> {
    routes.iter().filter(|r| r.path == path).map(|r| r.method.clone()).collect()
}

#[test]
fn via_list_registers_one_route_per_verb() {
    let r = routes(r#"  match "widget_lookup/:id", to: "widgets#show", via: %i[get post]"#);
    assert_eq!(verbs(&r, "/widget_lookup/:id"), vec![HttpMethod::Get, HttpMethod::Post], "{r:?}");
    assert!(r.iter().all(|x| x.controller.0.as_str() == "WidgetsController"));
}

#[test]
fn via_single_verb_and_string_spelling() {
    let r = routes(
        r#"
  match "widget_ping", to: "widgets#ping", via: :post
  match "widget_pong", to: "widgets#pong", via: ["get", "delete"]
"#,
    );
    assert_eq!(verbs(&r, "/widget_ping"), vec![HttpMethod::Post], "{r:?}");
    assert_eq!(verbs(&r, "/widget_pong"), vec![HttpMethod::Get, HttpMethod::Delete], "{r:?}");
}

#[test]
fn duplicate_via_takes_the_last_value_as_ruby_does() {
    let r = routes(r#"  match "widget_edit", to: "widgets#edit", via: :get, via: :post"#);
    assert_eq!(verbs(&r, "/widget_edit"), vec![HttpMethod::Post], "{r:?}");
}

#[test]
fn via_all_stays_one_any_route() {
    let r = routes(r#"  match "widget_any/:id", to: "widgets#show", via: :all"#);
    assert_eq!(verbs(&r, "/widget_any/:id"), vec![HttpMethod::Any], "{r:?}");
}

fn ingest_error(source: &str) -> String {
    ingest_routes(
        format!("Rails.application.routes.draw do\n{source}\nend\n").as_bytes(),
        "config/routes.rb",
    )
    .map(|t| format!("ingested: {t:?}"))
    .expect_err("route should be unsupported")
    .to_string()
}

#[test]
fn via_verb_the_table_cannot_hold_is_unsupported_not_any() {
    let err = ingest_error(r#"  match "widget_trace", to: "widgets#trace", via: :trace"#);
    assert!(err.contains("via: :trace"), "{err}");
    let err = ingest_error(r#"  match "widget_mixed", to: "widgets#show", via: %i[get trace]"#);
    assert!(err.contains("via: :trace"), "{err}");
}

#[test]
fn non_literal_or_missing_via_is_unsupported_not_any() {
    let err = ingest_error(r#"  match "widget_verbs", to: "widgets#show", via: WIDGET_VERBS"#);
    assert!(err.contains("non-literal `via:`"), "{err}");
    let err = ingest_error(r#"  match "widget_verbs", to: "widgets#show", via: [:get, verb]"#);
    assert!(err.contains("non-literal `via:`"), "{err}");
    let err = ingest_error(r#"  match "widget_bare", to: "widgets#show""#);
    assert!(err.contains("without `via:`"), "{err}");
}

// Rails 8.1.4 builds `VerbMatchers::All` only for the symbol `:all`
// (anywhere in the list); `"all"` and `:ALL` become `Unknown("ALL")`,
// which no request matches, while `:GET` still answers GET.
#[test]
fn only_the_symbol_all_means_every_verb() {
    let r = routes(
        r#"
  match "widget_all_get", to: "widgets#show", via: [:all, :get]
  match "widget_shout", to: "widgets#show", via: :GET
"#,
    );
    assert_eq!(verbs(&r, "/widget_all_get"), vec![HttpMethod::Any], "{r:?}");
    assert_eq!(verbs(&r, "/widget_shout"), vec![HttpMethod::Get], "{r:?}");
    for (via, spelled) in [
        (r#""all""#, r#"via: "all""#),
        (":ALL", "via: :ALL"),
        (r#"[:get, "all"]"#, r#"via: "all""#),
    ] {
        let err = ingest_error(&format!(r#"  match "widget_odd", to: "widgets#show", via: {via}"#));
        assert!(err.contains(spelled), "{via}: {err}");
    }
}

#[test]
fn via_inside_a_member_block_keeps_the_member_scope() {
    let r = routes(
        r#"
  resources :widgets, only: [] do
    member do
      match "archive", to: "widgets#archive", via: %i[get patch]
    end
  end
"#,
    );
    assert_eq!(
        verbs(&r, "/widgets/:id/archive"),
        vec![HttpMethod::Get, HttpMethod::Patch],
        "{r:?}"
    );
}

#[test]
fn emitted_router_dispatches_match_via_verbs() {
    emit_and_run::real_blog()
        .write(
            "config/routes.rb",
            r#"
Rails.application.routes.draw do
  root "articles#index"
  match "lookup/:id", to: "articles#show", via: %i[get post]
  match "any/:id", to: "articles#edit", via: :all
  resources :articles do
    resources :comments, only: [:create, :destroy]
  end
end
"#,
        )
        .run_ruby(
            r##"
table = RouteTable.table
[
  ["GET", "/lookup/7", :show],
  ["POST", "/lookup/7", :show],
  ["GET", "/any/7", :edit],
  ["DELETE", "/any/7", :edit],
].each do |verb, path, action|
  matched = ActionDispatch::Router.match(verb, path, table)
  raise "no route for #{verb} #{path}" if matched.nil?
  raise "#{verb} #{path}: #{matched.action}" unless
    matched.action == action && matched.path_params == {"id" => "7"}
end
raise "PATCH /lookup/7 is routed" unless ActionDispatch::Router.match("PATCH", "/lookup/7", table).nil?
puts "PASS emitted match via:"
"##,
        )
        .assert_passes();
}
