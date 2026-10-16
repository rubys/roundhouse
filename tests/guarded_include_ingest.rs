//! A class-body `include` guarded by `begin … rescue NameError … end`.
//!
//! ```ruby
//! class Part < ApplicationRecord
//!   begin
//!     include(Chime)
//!   rescue NameError
//!   end
//! end
//! ```
//!
//! is the "include it when the module is there" idiom. The model body
//! walk only knew calls, so the `begin` was an unknown item that no
//! later pass read: the include never reached the class, and the
//! first call to one of the module's methods raised `NoMethodError`.
//! Now the guarded body is the class's own declarations when every
//! module it names is defined in the app, and nothing when one is not
//! (in Rails the rescue swallows that `NameError`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "parts", force: :cascade do |t|
    t.string "name"
  end
end
"#;

const CHIME: &str = "module Chime\n  def ring(code)\n    \"rang #{code}\"\n  end\nend\n";

fn part(head: &str) -> String {
    format!(
        "class Part < ApplicationRecord\n{head}\n  def announce\n    ring(\"part\")\n  end\nend\n"
    )
}

/// The emitted `part` model files for a model body opening with `head`.
fn emitted_part(target: BuildTarget, head: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/concerns/chime.rb"), CHIME.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/part.rb"), part(head).into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    let mut parts: Vec<(String, String)> = files
        .into_iter()
        .filter(|(p, _)| p.ends_with("models/part.rb") || p.ends_with("models/part.rbs"))
        .collect();
    assert!(!parts.is_empty(), "no part model emitted");
    parts.sort();
    // Source-line markers name the model's own lines, which the block
    // above them shifts; the code is what is compared.
    parts
        .into_iter()
        .map(|(p, c)| {
            let code: Vec<&str> = c.lines().filter(|l| !l.starts_with("#<SPINEL_SOURCE>")).collect();
            format!("# {p}\n{}", code.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const PLAIN: &str = "  include Chime\n";
const GUARDED: &str = "  begin\n    include(Chime)\n  rescue NameError\n  end\n";
const GUARDED_LOAD: &str = "  begin\n    include Chime\n  rescue NameError, LoadError\n    nil\n  end\n";
/// `NameError`'s ancestors catch it too, and a bare `rescue` is
/// `StandardError`.
const GUARDED_STANDARD: &str = "  begin\n    include Chime\n  rescue StandardError\n  end\n";
const GUARDED_BARE: &str = "  begin\n    include Chime\n  rescue\n  end\n";
/// Absolute const path — `::Chime` is root-only lookup, not `"::Chime"`.
const GUARDED_ABSOLUTE: &str = "  begin\n    include ::Chime\n  rescue NameError\n  end\n";

#[test]
fn a_guarded_include_of_a_defined_module_is_the_include() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let plain = emitted_part(target, PLAIN);
        assert!(plain.contains("ring"), "{target:?}: the plain include reaches the model:\n{plain}");
        for guarded in [GUARDED, GUARDED_LOAD, GUARDED_STANDARD, GUARDED_BARE, GUARDED_ABSOLUTE] {
            assert_eq!(emitted_part(target, guarded), plain, "{target:?}: {guarded}");
        }
    }
}

/// A module the app does not define: Rails' rescue swallows the
/// `NameError`, and the class is what it would be without the block.
#[test]
fn a_guarded_include_of_an_undefined_module_is_nothing() {
    let missing = "  begin\n    include(Bell)\n  rescue NameError\n  end\n";
    let first = "  begin\n    include Bell\n    include Chime\n  rescue NameError\n  end\n";
    let together = "  begin\n    include Chime, Bell\n  rescue NameError\n  end\n";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let bare = emitted_part(target, "");
        for source in [missing, first, together] {
            assert_eq!(emitted_part(target, source), bare, "{target:?}: {source}");
        }
    }
}

/// Ruby runs the includes in order and stops at the first that raises:
/// the ones before it stay.
#[test]
fn the_includes_before_the_undefined_one_stay() {
    let partly = "  begin\n    include Chime\n    include Bell\n  rescue NameError\n  end\n";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        assert_eq!(emitted_part(target, partly), emitted_part(target, PLAIN), "{target:?}");
    }
}

/// Blocks this pass does not model are left as they were: a rescue that
/// cannot catch the `NameError` (`LoadError` is a `ScriptError`), and a
/// rescue that does something (a fallback include).
#[test]
fn other_guards_are_left_alone() {
    let load_only = "  begin\n    include Chime\n  rescue LoadError\n  end\n";
    let fallback = "  begin\n    include Bell\n  rescue NameError\n    include Chime\n  end\n";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        for source in [load_only, fallback] {
            let src = emitted_part(target, source);
            assert!(!src.contains("include Chime"), "{target:?}: {source}\n{src}");
        }
    }
}

/// Core modules live outside the app set but are still defined in Ruby.
#[test]
fn a_guarded_include_of_enumerable_is_kept() {
    let guarded = "  begin\n    include Enumerable\n  rescue NameError\n  end\n";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let src = emitted_part(target, guarded);
        assert!(
            src.contains("include Enumerable") || src.contains("Enumerable"),
            "{target:?}: core module must not be dropped as missing:\n{src}"
        );
    }
}

/// A successful module include before a class target must stay — same
/// prefix contract as an undefined name after a defined module.
#[test]
fn the_includes_before_a_class_target_stay() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/concerns/chime.rb"), CHIME.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/gong.rb"),
        b"class Gong < ApplicationRecord\nend\n".to_vec(),
    );
    let head = "  begin\n    include Chime\n    include Gong\n  rescue StandardError\n  end\n";
    tree.insert(PathBuf::from("app/models/part.rb"), part(head).into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.ends_with("models/part.rb"))
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            src.contains("def ring") || src.contains("include Chime"),
            "{target:?}: Chime prefix must stay:\n{src}"
        );
        assert!(!src.contains("include Gong"), "{target:?}: class must not splice:\n{src}");
    }
}

/// `include` of a class raises TypeError; do not splice it.
#[test]
fn a_guarded_include_of_a_class_is_left_alone() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/gong.rb"),
        b"class Gong < ApplicationRecord\nend\n".to_vec(),
    );
    let head = "  begin\n    include Gong\n  rescue StandardError\n  end\n";
    tree.insert(PathBuf::from("app/models/part.rb"), part(head).into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.ends_with("models/part.rb"))
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !src.contains("include Gong"),
            "{target:?}: class include must not be spliced:\n{src}"
        );
    }
}

/// A class target under `rescue NameError` raises unrescued `TypeError`
/// in Ruby — leave the begin alone rather than erasing the failure.
#[test]
fn a_class_target_under_name_error_rescue_is_left_alone() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/concerns/chime.rb"), CHIME.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/gong.rb"),
        b"class Gong < ApplicationRecord\nend\n".to_vec(),
    );
    // Prefix would be wrong to splice: Ruby includes Chime then TypeErrors.
    let head = "  begin\n    include Chime\n    include Gong\n  rescue NameError\n  end\n";
    tree.insert(PathBuf::from("app/models/part.rb"), part(head).into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.ends_with("models/part.rb"))
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !src.contains("def ring") && !src.contains("include Chime"),
            "{target:?}: must not splice Chime when a later class target is unrescued:\n{src}"
        );
        assert!(!src.contains("include Gong"), "{target:?}: class must not splice:\n{src}");
    }
}

/// A module whose residual `def self.included` can raise keeps its guard.
#[test]
fn a_module_with_included_hook_keeps_the_guard() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/concerns/boom.rb"),
        b"module Boom\n  def self.included(base)\n    raise NameError, \"boom\"\n  end\n  def bang\n    \"bang\"\n  end\nend\n".to_vec(),
    );
    let head = "  begin\n    include Boom\n  rescue NameError\n  end\n";
    tree.insert(
        PathBuf::from("app/models/part.rb"),
        format!(
            "class Part < ApplicationRecord\n{head}\n  def announce\n    \"ok\"\n  end\nend\n"
        )
        .into_bytes(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.ends_with("models/part.rb"))
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // Bare-splicing would raise outside the rescue. Leaving the begin
        // means the include never reaches the class (same as before this
        // pass modeled the form) — not a silent bare include.
        assert!(
            !src.contains("include Boom") && !src.contains("bang"),
            "{target:?}: must not bare-splice a module with included hook:\n{src}"
        );
    }
}

/// Relative `Chime` inside `A::B` finds `A::B::Chime` (the model's own
/// nest), without inventing `A::Chime`.
#[test]
fn relative_include_resolves_in_the_model_nest() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/a/b/chime.rb"),
        b"module A\n  module B\n    module Chime\n      def ring(code)\n        \"rang #{code}\"\n      end\n    end\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/a/b.rb"),
        b"class A::B < ApplicationRecord\n  begin\n    include Chime\n  rescue NameError\n  end\n  def announce\n    ring(\"part\")\n  end\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.contains("models") && (p.ends_with("b.rb") || p.contains("/a/")))
            .map(|(p, c)| format!("# {p}\n{c}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            src.contains("def ring") || src.contains("include Chime") || src.contains("include A::B::Chime"),
            "{target:?}: A::B::Chime must resolve for include Chime inside A::B:\n{src}"
        );
    }
}

/// `class A::B` does not open `A` lexically — do not resolve `Chime` to
/// `A::Chime` from the qualified class name.
#[test]
fn qualified_class_name_does_not_invent_lexical_scope() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/a/chime.rb"),
        b"module A\n  module Chime\n    def ring(code)\n      \"rang #{code}\"\n    end\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/a/b.rb"),
        b"class A::B < ApplicationRecord\n  begin\n    include Chime\n  rescue NameError\n  end\n  def announce\n    \"ok\"\n  end\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
        let src = files
            .iter()
            .filter(|(p, _)| p.contains("models") && (p.ends_with("b.rb") || p.contains("/a/")))
            .map(|(p, c)| format!("# {p}\n{c}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !src.contains("include Chime") && !src.contains("include A::Chime"),
            "{target:?}: must not invent A:: scope from class A::B:\n{src}"
        );
    }
}
