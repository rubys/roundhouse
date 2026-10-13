//! Source-backed Ruby constant resolution is determined at the reference
//! site, not by the body's inferred `self` class or a same-suffix registry
//! entry. Qualified enum members and classes must retain their declaration
//! owner, while names outside that lexical scope stay unresolved.
use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

/// A stage enum member and a same-named class in another namespace.
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

const LEXICAL_SCOPES: &str = r#"module Lexical
  class Target
  end
  module Nested
    VALUE = Target
  end
end
class Lexical::Explicit
  VALUE = Target
end
module Elsewhere
  class Target
  end
end
"#;

fn app_with(action: &str) -> roundhouse::App {
    app_with_sources(action, None)
}

fn app_with_lexical_scope(action: &str) -> roundhouse::App {
    app_with_sources(action, Some(("app/services/lexical_scope.rb", LEXICAL_SCOPES)))
}

fn app_with_sources(action: &str, extra_source: Option<(&str, &str)>) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
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
    .map(|(path, content)| (PathBuf::from(path), content.into_bytes()))
    .collect();
    if let Some((path, source)) = extra_source {
        tree.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
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
fn a_bare_model_read_uses_its_source_declaration() {
    let app = app_with_sources(
        "@a = Article.new",
        Some(("app/models/article.rb", "class Article < ApplicationRecord\nend\n")),
    );
    assert_eq!(
        ty_at(&app, 2, 9),
        Some(Ty::Class { id: ClassId(Symbol::from("Article")), args: vec![].into() }),
    );
}

#[test]
fn a_qualified_class_read_is_the_class_not_a_same_named_constant() {
    let app = app_with("@a = Workspace::Drafting::Drafting.new");
    // Line 2 (0-based), the constant starts after `    @a = `.
    let ty = ty_at(&app, 2, 9).expect("the constant is typed");
    assert_eq!(
        ty,
        Ty::Class { id: ClassId(Symbol::from("Workspace::Drafting::Drafting")), args: vec![].into() },
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
        Ty::Class { id: ClassId(Symbol::from("Core::StageEnum")), args: vec![].into() },
        "an enum member is an instance of its enum"
    );
}

#[test]
fn source_references_use_ruby_lexical_nesting_not_the_body_self_class() {
    let app = app_with_lexical_scope(
        "@nested = Lexical::Nested::VALUE\n    @explicit = Lexical::Explicit::VALUE",
    );
    assert_eq!(
        ty_at(&app, 2, 14),
        Some(Ty::Class { id: ClassId(Symbol::from("Lexical::Target")), args: vec![].into() }),
        "a nested module block includes Lexical in its constant nesting"
    );
    assert!(
        matches!(ty_at(&app, 3, 16), Some(Ty::Var { .. })),
        "`class Lexical::Explicit` does not lexically nest inside Lexical"
    );
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == roundhouse::diagnostic::Severity::Error
                && diagnostic.code() == "unsupported"
                && diagnostic.message.contains("Lexical::Explicit::VALUE")
        }),
        "the unrelated Elsewhere::Target must not capture the unresolved reference: {diagnostics:?}"
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
        Some(Ty::Class { id: ClassId(Symbol::from("UI::Selector::Mode")), args: vec![].into() }),
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

#[test]
fn an_absolute_class_uses_the_same_registry_identity_as_a_bare_class() {
    let app = app_with("@a = ::Time.now");
    assert_eq!(
        ty_at(&app, 2, 16),
        Some(Ty::Class { id: ClassId(Symbol::from("Time")), args: vec![].into() }),
    );
}

#[test]
fn an_absolute_nested_constant_keeps_its_root_and_resolves_its_owner() {
    let app = app_with("@a = ::Core::StageEnum::Drafting");
    assert_eq!(
        ty_at(&app, 2, 9),
        Some(Ty::Class { id: ClassId(Symbol::from("Core::StageEnum")), args: vec![].into() }),
    );
    let parsed = ruby_prism::parse(b"::Core::StageEnum::Drafting");
    let expr = roundhouse::ingest::ingest_expr(&parsed.node().as_program_node().unwrap().statements().as_node(), "root.rb")
        .expect("ingest");
    assert_eq!(roundhouse::emit::ruby::emit_expr(&expr), "::Core::StageEnum::Drafting");
}

#[test]
fn ingested_source_apps_refuse_ir_only_mode_after_source_table_loss() {
    let app = app_with("    @value = Core::StageEnum::Drafting");
    assert!(app.source_index_required);
    assert!(!app.sources.is_empty());
    // The contract survives IR serialization; the prepared cache need not.
    let serialized = serde_json::to_string(&app).unwrap();
    let mut restored: roundhouse::App = serde_json::from_str(&serialized).unwrap();
    restored.sources.clear();
    let refusal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        roundhouse::analyze::Analyzer::new(&restored);
    })).expect_err("a source app must not become an IR-only API app");
    let message = refusal.downcast_ref::<String>().map(String::as_str)
        .or_else(|| refusal.downcast_ref::<&str>().copied()).unwrap();
    assert!(message.contains("source_index_missing"), "{message}");
}

#[test]
fn an_empty_source_project_needs_no_source_index() {
    let app = ingest_app_from_tree(HashMap::new()).expect("empty project ingests");
    assert!(app.sources.is_empty());
    assert!(!app.source_index_required);
    roundhouse::analyze::Analyzer::new(&app);
}
