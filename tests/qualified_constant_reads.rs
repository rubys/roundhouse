//! `A::B::Name` means `Name` in the namespace `A::B` — not whatever
//! `Name` happens to mean somewhere else in the app.
//!
//! `ExprNode::Const` looked up the LAST path segment in the global
//! by-bare-name constant registry before considering the path at all.
//! One constant named `Drafting` anywhere in the app therefore
//! captured every qualified read ending in `::Drafting`, including
//! the class of that name — its reads typed as the other constant's
//! value, and everything downstream of them went untyped.
//!
//! An app hits this as soon as a constant shares a name with a class,
//! which a `T::Enum` makes easy: its members are constants, and an
//! enum of subsystem names is exactly a list of class names.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

/// A stage enum with a member named `Drafting`, and a class of
/// that name in another namespace — the shape from the app this came
/// from.
const TRACKING: &str = r#"module Core
  class StageEnum < T::Enum
    enums do
      Drafting = new("drafting")
      Reviewing = new("reviewing")
    end
  end
end
"#;

const DRAFTING: &str = r#"module Workspace
  module Drafting
    class Drafting
      def queries
        "queries"
      end
    end
  end
end
"#;

fn app_with(action: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_string(),
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string()),
        ("app/services/stage_enum.rb", TRACKING.to_string()),
        ("app/services/drafting.rb", DRAFTING.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        (
            "app/controllers/reports_controller.rb",
            format!("class ReportsController < ApplicationController\n  def show\n    {action}\n  end\nend\n"),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn ty_at(app: &roundhouse::App, line: u32, character: u32) -> Option<Ty> {
    roundhouse::ide::type_at_position(
        app,
        "app/controllers/reports_controller.rb",
        roundhouse::ide::Position { line, character },
    )
    .and_then(|t| t.ty)
}

#[test]
fn a_qualified_class_read_is_the_class_not_a_same_named_constant() {
    let app = app_with("@a = Workspace::Drafting::Drafting.new");
    // Line 2 (0-based), the constant starts after `    @a = `.
    let ty = ty_at(&app, 2, 9).expect("the constant is typed");
    assert_eq!(
        ty,
        Ty::Class { id: ClassId(Symbol::from("Workspace::Drafting::Drafting")), args: vec![] },
        "a read of the class must be the class, not `Core::StageEnum`'s member of the same name"
    );
}

#[test]
fn a_qualified_constant_read_still_resolves_through_its_owner() {
    // The other half: the enum member itself still types, by asking the
    // class that owns it.
    let app = app_with("@a = Core::StageEnum::Drafting");
    let ty = ty_at(&app, 2, 9).expect("the constant is typed");
    assert_eq!(
        ty,
        Ty::Class { id: ClassId(Symbol::from("Core::StageEnum")), args: vec![] },
        "an enum member is an instance of its enum"
    );
}

#[test]
fn the_owner_is_resolved_the_way_ruby_resolves_it() {
    // `Mode::Fill` written inside the class that declares `Mode` means
    // that class's `Mode` — which is the only thing that tells a dozen
    // components' `Mode` enums apart. A suffix match over the class
    // registry cannot, and this app has two.
    let nested = r#"module UI
  class Selector
    class Mode < T::Enum
      enums do
        Fill = new("fill")
      end
    end
    DEFAULT_MODE = Mode::Fill
  end

  class Other
    class Mode < T::Enum
      enums do
        Off = new("off")
      end
    end
  end
end
"#;
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_string(),
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string()),
        ("app/services/ui.rb", nested.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        (
            "app/controllers/reports_controller.rb",
            "class ReportsController < ApplicationController\n  def show\n    @a = UI::Selector::DEFAULT_MODE.serialize\n  end\nend\n".to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);

    // The member is an instance of ITS enum, and `serialize` answers
    // the type its members were built from.
    assert_eq!(
        ty_at(&app, 2, 10),
        Some(Ty::Class { id: ClassId(Symbol::from("UI::Selector::Mode")), args: vec![] }),
        "`DEFAULT_MODE` is `UI::Selector::Mode`, not `UI::Other::Mode`"
    );
    let diagnostics: Vec<String> = roundhouse::analyze::diagnose(&app)
        .iter()
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect();
    assert!(
        !diagnostics.iter().any(|d| d.contains("send_dispatch_failed")),
        "`serialize` dispatches against the enum; diagnostics = {diagnostics:?}"
    );
}
