//! Relation typing vs delegated_type singular readers.
//!
//! `Leaf#page` (from `delegated_type :leafable, types: …Page…`) must stay
//! a record reader — not collide with Relation/`ActiveRecord::Base`
//! pagination `page` that returns `Relation[Leaf]`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "leaves", force: :cascade do |t|
    t.string "leafable_type", null: false
    t.integer "leafable_id", null: false
    t.string "title", null: false
  end
  create_table "pages", force: :cascade do |t|
  end
  create_table "sections", force: :cascade do |t|
    t.text "body"
  end
  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
  end
end
"#,
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/models/page.rb",
            "class Page < ApplicationRecord\n  has_markdown :body\nend\n",
        ),
        (
            "app/models/section.rb",
            "class Section < ApplicationRecord\nend\n",
        ),
        (
            "app/models/leaf.rb",
            r#"class Leaf < ApplicationRecord
  delegated_type :leafable, types: %w[ Page Section ]
end
"#,
        ),
        (
            "app/controllers/leaves_controller.rb",
            r#"class LeavesController < ApplicationController
  def show
    @leaf = Leaf.find(params[:id])
    render :show
  end
end
"#,
        ),
        (
            "app/views/leaves/show.html.erb",
            "<%= @leaf.page.body.to_s %>\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :leaves\nend\n",
        ),
    ]))
    .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn ty_of_send_named(app: &roundhouse::App, view_name: &str, method: &str) -> Option<Ty> {
    let v = app.views.iter().find(|v| v.name.as_str() == view_name)?;
    fn walk(e: &roundhouse::expr::Expr, method: &str, out: &mut Option<Ty>) {
        if let ExprNode::Send { method: m, .. } = &*e.node {
            if m.as_str() == method {
                *out = e.ty.clone();
            }
        }
        e.node.for_each_child(&mut |c| walk(c, method, out));
    }
    let mut out = None;
    walk(&v.body, method, &mut out);
    out
}

#[test]
fn leaf_page_reader_is_not_relation_leaf() {
    let app = app();
    let page_ty = ty_of_send_named(&app, "leaves/show", "page");
    eprintln!("@leaf.page ty = {page_ty:?}");
    assert!(
        !matches!(&page_ty, Some(Ty::Relation { of }) if of.0.as_str() == "Leaf"),
        "@leaf.page must not be Relation[Leaf], got {page_ty:?}"
    );
    // Narrowed delegated_type singular reader: Page | nil — not the
    // full polymorphic leafable union and not Relation[Leaf].
    match &page_ty {
        Some(Ty::Union { variants }) => {
            assert_eq!(variants.len(), 2, "expected Page | nil, got {page_ty:?}");
            assert!(
                variants
                    .iter()
                    .any(|v| matches!(v, Ty::Class { id, .. } if id.0.as_str() == "Page")),
                "union must include Page: {page_ty:?}"
            );
            assert!(
                variants.iter().any(|v| matches!(v, Ty::Nil)),
                "union must include Nil: {page_ty:?}"
            );
            assert!(
                !variants.iter().any(|v| matches!(v, Ty::Relation { .. })),
                "union must not include Relation: {page_ty:?}"
            );
            assert!(
                !variants
                    .iter()
                    .any(|v| matches!(v, Ty::Class { id, .. } if id.0.as_str() == "Section")),
                "singular page reader must not widen to Section: {page_ty:?}"
            );
        }
        Some(Ty::Class { id, .. }) if id.0.as_str() == "Page" => {}
        other => panic!("expected Page | nil, got {other:?}"),
    }
}

#[test]
fn leaf_page_body_dispatches_without_relation_error() {
    let app = app();
    let diags: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| d.message.contains("`body`") && d.message.contains("Relation"))
        .map(|d| d.message.clone())
        .collect();
    assert!(
        diags.is_empty(),
        "no body-on-Relation errors expected, got {diags:?}"
    );
}
