//! `include(WithIds)` names a module nested in the class's superclass.
//!
//! Past the lexical scopes Ruby searches the class's ancestors before
//! the top level. `Child::Thing < Base::Entity` opening with
//! `include(WithIds)` means `Base::Entity::WithIds`.
//! The include was searched only lexically, so it stayed bare and was
//! refused as `includes unresolved WithIds`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const ENTITY: &str = "module Base\n  class Entity\n  end\nend\n";

const WITH_IDS: &str = r#"
module Base
  class Entity
    module WithIds
      #: -> Integer
      def ident = 1
    end
  end
end
"#;

const THING: &str = r#"
module Child
  class Thing < Base::Entity
    include(WithIds)
  end

  # A grandchild finds it two steps up the chain.
  class ThingChild < Thing
    include(WithIds)
  end
end
"#;

/// A `WithIds` nested in the class itself shadows the superclass's.
const SHADOWED: &str = r#"
module Child
  class Shadowed < Base::Entity
    module WithIds
      #: -> Integer
      def other = 2
    end

    include(WithIds)
  end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/base/entity.rb", ENTITY),
        ("app/models/base/entity/with_ids.rb", WITH_IDS),
        ("app/models/child/thing.rb", THING),
        ("app/models/child/shadowed.rb", SHADOWED),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn includes(app: &roundhouse::App, class: &str) -> Vec<String> {
    app.library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == class)
        .unwrap_or_else(|| panic!("no library class {class}"))
        .includes
        .iter()
        .map(|c| c.0.as_str().to_string())
        .collect()
}

#[test]
fn an_include_resolves_through_the_superclass() {
    let app = app();
    assert_eq!(includes(&app, "Child::Thing"), ["Base::Entity::WithIds"]);
    assert_eq!(includes(&app, "Child::ThingChild"), ["Base::Entity::WithIds"]);
    let errors: Vec<String> = diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(!errors.iter().any(|e| e.contains("includes unresolved")), "{errors:?}");
}

/// The lexical scopes are searched first: a `WithIds` nested in the
/// class itself wins over the superclass's.
#[test]
fn a_lexical_definition_wins_over_the_superclass() {
    assert_eq!(includes(&app(), "Child::Shadowed"), ["Child::Shadowed::WithIds"]);
}

const SCRIPT: &str = r#"
p [Child::Thing.new.ident, Child::ThingChild.new.ident]
p Child::Thing.include?(Base::Entity::WithIds)
p Child::Shadowed.new.respond_to?(:ident)
"#;

const EXPECTED: &str = "[1, 1]\ntrue\nfalse\n";

/// The include loads and the module's method is callable on the emitted
/// classes, as on native Ruby.
#[test]
fn the_superclass_nested_include_runs_as_on_native_ruby() {
    let sources = [ENTITY, WITH_IDS, THING, SHADOWED].join("\n");
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("{sources}\n{SCRIPT}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED);
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/base/entity.rb", ENTITY)
        .write("app/models/base/entity/with_ids.rb", WITH_IDS)
        .write("app/models/child/thing.rb", THING)
        .write("app/models/child/shadowed.rb", SHADOWED)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}
