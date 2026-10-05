//! `presence` on a receiver typed `T` is `T?`, not `untyped`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, DiagnosticKind};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn app_from(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    app
}

#[test]
fn presence_on_a_typed_receiver_is_that_type_or_nil() {
    let app = app_from(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def show\n    render plain: label_for(\"draft\")\n  end\n\n  private\n\n  def label_for(name)\n    (title = name.presence) && title.upcase\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/articles/:id\", to: \"articles#show\"\nend\n",
        ),
    ]);
    let gradual: Vec<String> = diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::GradualUntyped { .. }
                    | DiagnosticKind::UnresolvedType { .. }
                    | DiagnosticKind::SendDispatchFailed { .. }
            )
        })
        .map(|d| d.message)
        .collect();
    assert!(
        gradual.is_empty(),
        "`title.presence` should type as String?, and `title.upcase` under the `&&` as String:\n{}",
        gradual.join("\n")
    );
    let mut presence = None;
    fn visit(expr: &roundhouse::Expr, presence: &mut Option<Ty>) {
        if let ExprNode::Send { method, .. } = &*expr.node {
            if method.as_str() == "presence" {
                *presence = expr.ty.clone();
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, presence));
    }
    for action in app.controllers.iter().flat_map(|c| c.actions()) {
        visit(&action.body, &mut presence);
    }
    match presence {
        Some(Ty::Union { variants }) => {
            assert!(
                variants.contains(&Ty::Str) && variants.contains(&Ty::Nil),
                "name.presence should be String?, got {variants:?}"
            );
        }
        other => panic!("name.presence should be String?, got {other:?}"),
    }
}

#[test]
fn presence_on_an_untyped_receiver_stays_untyped() {
    // `Untyped | Nil` is not the same as `Untyped`: later inference
    // treats the union as a known type and reports missing methods on it.
    let app = app_from(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n  def show\n    render plain: maybe.presence\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/articles/:id\", to: \"articles#show\"\nend\n",
        ),
    ]);
    let kinds: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::SendDispatchFailed { .. }))
        .map(|d| d.message)
        .collect();
    assert!(
        kinds.is_empty(),
        "untyped.presence must not become Untyped | Nil:\n{}",
        kinds.join("\n")
    );
}
