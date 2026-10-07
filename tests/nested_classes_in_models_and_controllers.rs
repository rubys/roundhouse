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

/// A controller concern with a nested `T::Struct` is a module file, not
/// a controller. The nested class does not end in `Controller`, and
/// falling back to it invented a fake controller whose `const` calls
/// became unrecognized class-body macros while the enclosing concern
/// was skipped entirely.
#[test]
fn a_t_struct_nested_in_a_controller_concern_is_not_a_controller() {
    use roundhouse::ingest::survey;

    survey::activate();
    let app = app_with(vec![
        (
            "app/controllers/concerns/window_settings.rb",
            r#"module WindowSettings
  extend ActiveSupport::Concern

  class Span < T::Struct
    const :from_date, Date
    const :to_date, Date
  end

  class_methods do
    def window_config(**opts)
      @window_options = opts
    end
  end

  def window
    Span.new(from_date: Date.new(2026, 1, 1), to_date: Date.new(2026, 1, 31))
  end
end
"#
            .to_string(),
        ),
        (
            "app/controllers/reports_controller.rb",
            "class ReportsController < ApplicationController\n  include WindowSettings\n  def show; end\nend\n"
                .to_string(),
        ),
    ]);
    let gaps = survey::drain();

    assert!(
        !app.controllers.iter().any(|c| c.name.0.as_str() == "WindowSettings::Span"),
        "the nested T::Struct must not become a controller; controllers = {:?}",
        app.controllers.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>()
    );
    assert!(
        library_class_names(&app).contains(&"WindowSettings".to_string()),
        "the concern module registers; library classes = {:?}",
        library_class_names(&app)
    );
    assert!(
        library_class_names(&app).contains(&"WindowSettings::Span".to_string()),
        "the nested struct registers under its qualified name; library classes = {:?}",
        library_class_names(&app)
    );
    let span = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "WindowSettings::Span")
        .expect("Span library class");
    assert!(
        span.methods.iter().any(|m| m.name.as_str() == "from_date"),
        "const lowers to a reader; methods = {:?}",
        span.methods.iter().map(|m| m.name.as_str().to_string()).collect::<Vec<_>>()
    );
    assert!(
        span.methods.iter().any(|m| m.name.as_str() == "initialize"),
        "const lowers to the keyword constructor; methods = {:?}",
        span.methods.iter().map(|m| m.name.as_str().to_string()).collect::<Vec<_>>()
    );
    let messages: Vec<_> = gaps.iter().map(ToString::to_string).collect();
    assert!(
        !messages.iter().any(|g| g.contains("macro not recognized: `const`")
            || g.contains("Sorbet `const`")),
        "const must not be a survey gap once the struct is lowered; gaps = {messages:?}"
    );
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "ReportsController"),
        "the real controller is still there"
    );
}

/// A bare `const` on a real `*Controller` is leftover Sorbet props with
/// no runtime in the emitted tree. The survey names that specifically,
/// rather than folding it into the generic unrecognized-macro bucket.
#[test]
fn leftover_const_on_a_real_controller_earns_a_sorbet_survey_line() {
    use roundhouse::ingest::survey;

    survey::activate();
    let app = app_with(vec![(
        "app/controllers/reports_controller.rb",
        "class ReportsController < ApplicationController\n  const :label, String\n  def show; end\nend\n"
            .to_string(),
    )]);
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "ReportsController"),
        "the controller still ingests"
    );
    let gaps = survey::drain();
    let messages: Vec<_> = gaps.iter().map(ToString::to_string).collect();
    assert!(
        messages.iter().any(|m| {
            m.contains("Sorbet `const` outside a lowered T::Struct")
                && m.contains("app/controllers/reports_controller.rb")
        }),
        "leftover const must earn the Sorbet-specific line with the source path: {messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("controller class-body macro not recognized: `const`")),
        "must not use the generic macro bucket: {messages:?}"
    );
}
