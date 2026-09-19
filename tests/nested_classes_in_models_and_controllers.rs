//! A class nested in a model's or controller's body.
//!
//! `class Row < T::Struct` inside a model is the same declaration it
//! would be in `app/services`, and the library-class ingest has always
//! handled it there — `Holder::Row` registers under its qualified name.
//! In a model or a controller the statement reached the EXPRESSION
//! ingester instead, which has no `ClassNode` arm, so strict ingest
//! aborted the whole file with "unsupported expression node" and survey
//! mode substituted nil for the class.
//!
//! A DTO nested in the class that answers it is ordinary Ruby, and it
//! is where an app with typed value objects puts them.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "reports", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;
const NESTED: &str = r#"  class Row < T::Struct
    const :name, String
  end
"#;

fn app_with(files: Vec<(&str, String)>) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.into_bytes());
    }
    ingest_app_from_tree(tree).expect("ingest")
}

fn library_class_names(app: &roundhouse::App) -> Vec<String> {
    app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect()
}

#[test]
fn a_class_nested_in_a_model_registers_under_its_qualified_name() {
    let app = app_with(vec![(
        "app/models/report.rb",
        format!("class Report < ApplicationRecord\n{NESTED}end\n"),
    )]);
    assert!(
        library_class_names(&app).contains(&"Report::Row".to_string()),
        "library classes = {:?}",
        library_class_names(&app)
    );
    assert_eq!(
        app.models.iter().filter(|m| m.name.0.as_str() == "Report").count(),
        1,
        "the model is still a model"
    );
    assert!(
        !library_class_names(&app).contains(&"Report".to_string()),
        "and it is not ALSO registered as a library class; got {:?}",
        library_class_names(&app)
    );
}

#[test]
fn a_class_nested_in_a_controller_registers_too() {
    let app = app_with(vec![(
        "app/controllers/reports_controller.rb",
        format!("class ReportsController < ApplicationController\n{NESTED}  def show; end\nend\n"),
    )]);
    assert!(
        library_class_names(&app).contains(&"ReportsController::Row".to_string()),
        "library classes = {:?}",
        library_class_names(&app)
    );
    assert!(
        !library_class_names(&app).contains(&"ReportsController".to_string()),
        "the controller is a controller, not a library class; got {:?}",
        library_class_names(&app)
    );
    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ReportsController")
        .expect("the controller survived");
    assert!(
        controller.body.iter().any(|item| matches!(
            item,
            roundhouse::dialect::ControllerBodyItem::Action { action, .. } if action.name.as_str() == "show"
        )),
        "its own action is still there"
    );
}

#[test]
fn a_nested_module_is_not_a_body_item_either() {
    let app = app_with(vec![(
        "app/models/report.rb",
        "class Report < ApplicationRecord\n  module Scopes\n    def self.recent\n      42\n    end\n  end\nend\n".to_string(),
    )]);
    assert!(
        library_class_names(&app).contains(&"Report::Scopes".to_string()),
        "library classes = {:?}",
        library_class_names(&app)
    );
}
