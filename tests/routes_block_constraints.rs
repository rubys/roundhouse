//! `constraints(param: /regex/) do … end` propagates its per-param
//! requirement onto every route it flattens to — transitively through
//! nested scopes and `resources` blocks — merging with a route's own
//! `constraints:` option the way Rails 8.1 does: the route's own key
//! wins, other keys union. Before this fix the block's own args were
//! parsed and then simply discarded at ingest (`ingest_route_body`'s
//! `"constraints"` passthrough flattened the block's children and
//! dropped the constraint), so a child route carried NO requirement at
//! all and matched any segment.

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

fn find<'a>(routes: &'a [FlatRoute], method: HttpMethod, path: &str) -> &'a FlatRoute {
    routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("missing {method:?} {path}: {routes:?}"))
}

#[test]
fn block_constraint_reaches_its_direct_child() {
    let r = routes(
        r#"
  constraints(slug: %r{[^@/.]+}) do
    get "/@:slug", to: "widgets#show"
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/@:slug");
    assert_eq!(
        show.constraints,
        vec![("slug".to_string(), "[^@/.]+".to_string())],
        "block-level constraint must ride the flattened route: {:?}",
        show.constraints
    );
}

#[test]
fn block_constraint_propagates_through_a_nested_scope() {
    let r = routes(
        r#"
  constraints(slug: %r{[^@/.]+}) do
    namespace :api do
      get "/@:slug", to: "widgets#show"
    end
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/api/@:slug");
    assert_eq!(
        show.constraints,
        vec![("slug".to_string(), "[^@/.]+".to_string())],
        "constraint must survive a nested namespace: {:?}",
        show.constraints
    );
}

#[test]
fn block_constraint_propagates_into_resources_member_actions_only() {
    // `:id` only appears on show/edit/update/destroy — index/new/create
    // carry no `:id` segment, so the block's `id:` requirement must not
    // land on them.
    let r = routes(
        r#"
  constraints(id: %r{[A-Z]\d+}) do
    resources :widgets
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/widgets/:id");
    assert_eq!(
        show.constraints,
        vec![("id".to_string(), "[A-Z]\\d+".to_string())],
        "resources member action must inherit the block constraint: {:?}",
        show.constraints
    );
    let index = find(&r, HttpMethod::Get, "/widgets");
    assert!(
        index.constraints.is_empty(),
        "index has no :id segment, so no constraint should land on it: {:?}",
        index.constraints
    );
}

#[test]
fn route_level_constraint_wins_over_the_enclosing_block_for_the_same_param() {
    let r = routes(
        r#"
  constraints(id: %r{[^/]+}) do
    get "/widgets/:id", to: "widgets#show", constraints: { id: /\d+/ }
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/widgets/:id");
    // `\d+` is the digit class — it rides `int_params`, not `constraints`.
    assert_eq!(
        show.int_params,
        vec!["id".to_string()],
        "the route's OWN constraints: wins over the block's: {:?} / {:?}",
        show.int_params,
        show.constraints
    );
    assert!(
        show.constraints.is_empty(),
        "the block's [^/]+ must not also apply once the route set its own id: {:?}",
        show.constraints
    );
}

#[test]
fn route_level_and_block_constraints_union_on_different_params() {
    let r = routes(
        r#"
  constraints(slug: %r{[^@/.]+}) do
    get "/@:slug/:tag", to: "widgets#show", constraints: { tag: /[^,.\/]+/ }
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/@:slug/:tag");
    let mut got = show.constraints.clone();
    got.sort();
    let mut want = vec![
        ("slug".to_string(), "[^@/.]+".to_string()),
        ("tag".to_string(), "[^,.\\/]+".to_string()),
    ];
    want.sort();
    assert_eq!(got, want, "distinct params union rather than overwrite: {got:?}");
}

#[test]
fn digit_class_block_constraint_still_rides_int_params() {
    let r = routes(
        r#"
  constraints(id: /\d+/) do
    get "/widgets/:id", to: "widgets#show"
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/widgets/:id");
    assert_eq!(
        show.int_params,
        vec!["id".to_string()],
        "a digit-class block constraint keeps working exactly as a route-level one does: {:?}",
        show.int_params
    );
    assert!(show.constraints.is_empty());
}

#[test]
fn nested_constraints_blocks_let_the_innermost_win() {
    let r = routes(
        r#"
  constraints(id: %r{[^/]+}) do
    constraints(id: %r{[A-Z]\d+}) do
      get "/widgets/:id", to: "widgets#show"
    end
  end
"#,
    );
    let show = find(&r, HttpMethod::Get, "/widgets/:id");
    assert_eq!(
        show.constraints,
        vec![("id".to_string(), "[A-Z]\\d+".to_string())],
        "the innermost block's requirement wins over the outer one: {:?}",
        show.constraints
    );
}
