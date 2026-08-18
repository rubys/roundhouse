//! A record reaching a route helper projects to `.id` — but ONLY where
//! the segment it fills holds an id.
//!
//! `controller_to_library::rewrites::rewrite_route_helpers` did it
//! shape-directed: any Ivar argument got `.id`. That is right for
//! `article_url(@article)` and wrong for campfire's
//! `join_url(@join_code)`, whose only segment is `:join_code` — a
//! string column. The emit called `.id` on a String and every test that
//! loaded the join page died on it.
//!
//! The route's own segment is the signal that works. A type stamp is
//! not: the ivar is assigned in `setup` and read in the test method,
//! and nothing types an ivar across that boundary.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::controller_to_library::{
    lower_controllers_with_arel_views_assocs_and_routes, LowerControllerOptions,
};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :articles\n  \
    get \"/join/:join_code\" => \"articles#join\", as: :join\n  \
    root \"articles#index\"\nend\n";

const CONTROLLER: &str = r#"
class ArticlesController < ApplicationController
  def show
    @article = Article.find(params[:id])
    redirect_to article_url(@article)
  end

  def join
    @join_code = params[:join_code]
    redirect_to join_url(@join_code)
  end
end
"#;

fn emitted(wire_segments: bool) -> String {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/articles_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let segments = roundhouse::lower::routes::helper_id_segments(&app);
    // The table is the point of the test — assert its shape directly
    // too, so a regression in route flattening is not mistaken for a
    // regression in the rewrite.
    assert_eq!(
        segments.get("join_path").map(|v| v.as_slice()),
        Some(&[false][..]),
        "`/join/:join_code` has one non-id segment: {segments:?}"
    );
    assert_eq!(
        segments.get("article_path").map(|v| v.as_slice()),
        Some(&[true][..]),
        "`/articles/:id` has one id segment: {segments:?}"
    );
    let classes = lower_controllers_with_arel_views_assocs_and_routes(
        &app.controllers,
        Vec::new(),
        LowerControllerOptions {
            schema: Some(&app.schema),
            route_id_segments: wire_segments.then_some(&segments),
            ..Default::default()
        },
    );
    format!("{classes:?}")
}

/// Counting `.id` sends is what makes this a real test rather than a
/// substring wish: BLIND (no segment table) projects both arguments,
/// WIRED projects only the id-shaped one. The difference is exactly the
/// `@join_code` site.
fn id_projections(out: &str) -> usize {
    out.matches("method: Symbol(\"id\")").count()
}

#[test]
fn a_non_id_segment_does_not_project_the_argument_to_its_id() {
    let blind = id_projections(&emitted(false));
    let wired = id_projections(&emitted(true));
    assert_eq!(
        wired,
        blind - 1,
        "wiring the segment table must drop exactly the @join_code projection \
         (blind {blind}, wired {wired})"
    );
}

/// The other half: an id segment still projects, which is what
/// `article_path(@article)` has always relied on — a table that
/// declined everything would pass the test above.
#[test]
fn an_id_segment_still_projects() {
    let out = emitted(true);
    assert!(
        out.contains("Ivar { name: Symbol(\"article\") }"),
        "the lowered body still reads @article:\n{out}"
    );
    assert!(id_projections(&out) >= 1, "`article_path(@article)` keeps its `.id`:\n{out}");
}
