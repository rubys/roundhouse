//! `form_with ..., builder: CustomFormBuilder` yields THAT builder.
//!
//! An app that adds field helpers on a `FormBuilder` subclass is a very
//! common Rails shape, and `builder:` is how the form reaches them. The
//! block param was bound to the stock `ActionView::Helpers::FormBuilder`
//! unconditionally, so every `f.my_field` read as a dispatch failure.
//!
//! The second test covers the half that survived the first fix: the
//! binding used to be written only when the MODEL's type was also known,
//! so a record assigned through a gem roundhouse does not model (Pundit's
//! `authorize`, here stood in for by an ivar the controller never
//! assigns) cost the app its own builder methods too. The builder and the
//! model are independent — either one alone is worth binding.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const BUILDER: &str = r#"class CustomFormBuilder < ActionView::Helpers::FormBuilder
  def text_input_field(name, **options)
    text_field(name, **options)
  end
end
"#;

fn app_with(view: &str, action_body: &str) -> roundhouse::App {
    let files: Vec<(&str, String)> = vec![
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n".to_string()),
        ("app/helpers/custom_form_builder.rb", BUILDER.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        (
            "app/controllers/users_controller.rb",
            format!("class UsersController < ApplicationController\n  def new\n{action_body}  end\nend\n"),
        ),
        ("app/views/users/new.html.erb", view.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n".to_string(),
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn dispatch_failures(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .iter()
        .filter(|d| d.code() == "send_dispatch_failed")
        .map(|d| format!("{d:?}"))
        .collect()
}

#[test]
fn a_custom_builders_own_field_helper_resolves() {
    let app = app_with(
        "<%= form_with model: @user, builder: CustomFormBuilder do |f| %>\n\
         <%= f.text_input_field :name %>\n\
         <% end %>\n",
        "    @user = User.new\n",
    );
    let failures = dispatch_failures(&app);
    assert!(
        failures.is_empty(),
        "`builder:` names the form's builder, so its helpers resolve: {failures:?}"
    );
}

#[test]
fn the_builder_is_honoured_when_the_models_type_is_unknown() {
    // `@user` is never assigned — the type is unknown, the way a record
    // assigned through an unmodelled gem is. The builder still holds.
    let app = app_with(
        "<%= form_with model: @user, builder: CustomFormBuilder do |f| %>\n\
         <%= f.text_input_field :name %>\n\
         <% end %>\n",
        "",
    );
    let failures = dispatch_failures(&app);
    assert!(
        failures.is_empty(),
        "an untyped model must not cost the app its builder: {failures:?}"
    );
}

#[test]
fn literal_keyword_producer_retains_builder_and_independent_model_binding() {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ty::Ty;
    fn builder_reads(e: &Expr, out: &mut Vec<Ty>) {
        if matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "f") {
            if let Some(ty) = &e.ty {
                out.push(ty.clone());
            }
        }
        e.node
            .for_each_child(&mut |child| builder_reads(child, out));
    }
    for (body, known_model) in [("    @user = User.new\n", true), ("", false)] {
        let app = app_with(
            "<%= form_with(**{model: @user, builder: CustomFormBuilder}) do |f| %>\n<%= f.text_input_field :name %>\n<% end %>\n",
            body,
        );
        let failures = dispatch_failures(&app);
        assert!(
            failures.is_empty(),
            "literal ** options retain custom builder: {failures:?}"
        );
        let mut reads = Vec::new();
        for view in &app.views {
            builder_reads(&view.body, &mut reads);
        }
        assert!(
            reads.iter().any(|ty| matches!(ty, Ty::Class { id, args }
            if id.0.as_str() == "CustomFormBuilder" && if known_model {
                matches!(args.as_slice(), [Ty::Class { id, .. }] if id.0.as_str() == "User")
            } else { args.is_empty() })),
            "builder/model binding: {reads:?}"
        );
    }
}
