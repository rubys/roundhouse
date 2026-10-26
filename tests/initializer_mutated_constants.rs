//! A constant an initializer appends to, replaces or reassigns (#1256).
//! Rails runs `config/initializers` before it loads models and draws
//! routes, so `validates :kind, inclusion: { in: Catalog::KINDS }` and
//! `constraints: { id: Regexp.union(Catalog::KINDS) }` read the value the
//! initializer left (`rails_reads_the_initializers_value`, activerecord
//! and actionpack 8.1.4). The constant fold read only the inputs, so it
//! emitted the original list. A boot source is now observed before the
//! folds: a constant it writes or mutates doesn't fold, and the
//! validation and the constraint are reported gaps. An initializer that
//! only reads the constant, defines a top-level constant of its own, or
//! calls the owner, leaves the fold alone.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::process::Command;

const CATALOG: &str = "module Catalog\n  KINDS = %w[news blog]\nend\n";
const WIDGET: &str = "class Widget < ApplicationRecord\n  validates :kind, inclusion: { in: Catalog::KINDS }\nend\n";
const KINDS_CONTROLLER: &str = "class KindsController < ApplicationController\n  def show\n    render plain: params[:id]\n  end\nend\n";
const ROUTE: &str = "  get \"/kinds/:id\", to: \"kinds#show\", constraints: { id: Regexp.union(Catalog::KINDS) }\n";

/// Each initializer that leaves `Catalog::KINDS` other than `%w[news blog]`.
const MUTATING: [&str; 6] = [
    "Catalog::KINDS << \"wiki\"\n",
    "Catalog.send(:remove_const, :KINDS)\nCatalog::KINDS = %w[wiki]\n",
    "Catalog::KINDS.replace(%w[wiki])\n",
    "Catalog.send(:remove_const, :KINDS)\nCatalog.const_set(:KINDS, %w[wiki])\n",
    // A receiverless `const_set` in the owner's body.
    "module Catalog\n  send(:remove_const, :KINDS)\n  const_set(:KINDS, %w[wiki])\nend\n",
    // Both through `send`.
    "Catalog.send(:remove_const, :KINDS)\nCatalog.send(:const_set, :KINDS, %w[wiki])\n",
];

/// Reads the constant and defines a top-level one of its own.
const READING: &str = "LIMIT = 5\nKIND_COUNT = Catalog::KINDS.size\n";

/// Calls the owner without touching its constant: a call on a class or
/// module is not a write to one of its constants.
const CALLING: &str = "Catalog.class_eval do\n  def self.label = \"catalog\"\nend\nCatalog.respond_to?(:label)\n";

fn app(initializer: Option<&str>, route: bool) -> emit_and_run::Overlay {
    let routes = format!("Rails.application.routes.draw do\n{}end\n", if route { ROUTE } else { "" });
    let app = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/lib/catalog.rb", CATALOG)
        .write("app/models/widget.rb", WIDGET)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/kinds_controller.rb", KINDS_CONTROLLER)
        .write("config/routes.rb", &routes);
    match initializer {
        Some(source) => app.write("config/initializers/catalog.rb", source),
        None => app,
    }
}

fn model(initializer: Option<&str>) -> String {
    let (emitted, errors) = app(initializer, false).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    std::fs::read_to_string(emitted.join("app/models/widget.rb")).expect("widget.rb")
}

#[test]
fn a_mutated_constant_does_not_fold_into_a_validation() {
    for initializer in MUTATING {
        let widget = model(Some(initializer));
        assert!(!widget.contains("[\"news\", \"blog\"]"), "{initializer}\n{widget}");
        let warnings = app(Some(initializer), false).ingest_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("validates :kind, inclusion:") && w.contains("not supported")),
            "{initializer}: {warnings:#?}"
        );
    }
}

#[test]
fn a_mutated_constant_does_not_fold_into_a_route_constraint() {
    for initializer in MUTATING {
        let refused = std::panic::catch_unwind(|| app(Some(initializer), true).ingest_warnings());
        let message = refused.expect_err("the constraint is refused").downcast::<String>().map(|m| *m).unwrap_or_default();
        assert!(message.contains("constraints: id"), "{initializer}: {message}");
    }
}

#[test]
fn an_initializer_that_only_reads_keeps_the_fold() {
    for initializer in [None, Some(READING), Some(CALLING)] {
        assert!(model(initializer).contains("[\"news\", \"blog\"].include?(@kind)"), "{initializer:?}");
        let (emitted, errors) = app(initializer, true).emit(roundhouse::project::BuildTarget::Ruby);
        assert!(errors.is_empty(), "{errors:#?}");
        let routes = std::fs::read_to_string(emitted.join("config/routes.rb")).expect("routes.rb");
        assert!(routes.contains("news") && routes.contains("blog"), "{initializer:?}\n{routes}");
    }
}

/// Rails, with each initializer applied before the model is defined and
/// the routes are drawn: the validation and the constraint read the
/// initializer's value, which the original list does not match.
#[test]
fn rails_reads_the_initializers_value() {
    let probe = r#"gem "activerecord", "8.1.4"; gem "actionpack", "8.1.4"; require "active_record"; require "action_controller"; require "sqlite3""#;
    if !Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord/actionpack 8.1.4 and sqlite3 not available");
        return;
    }
    let expected = [("news", true, true), ("news", false, false), ("news", false, false), ("news", false, false), ("news", false, false), ("news", false, false)];
    for (initializer, (kind, news_valid, news_routed)) in MUTATING.iter().zip(expected) {
        let script = format!(
            r##"gem "activerecord", "8.1.4"
gem "actionpack", "8.1.4"
require "active_record"
require "action_controller"
require "rack/mock"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:widgets) {{ |t| t.string :kind }}
end
class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
class ApplicationController < ActionController::Base; end
{CATALOG}
{initializer}
{WIDGET}
{KINDS_CONTROLLER}
routes = ActionDispatch::Routing::RouteSet.new
routes.draw do
{ROUTE}end
status = ->(id) {{ routes.call(Rack::MockRequest.env_for("/kinds/#{{id}}"))[0] }}
got = [Widget.new(kind: "wiki").valid?, Widget.new(kind: {kind:?}).valid?, status.("wiki"), status.({kind:?})]
want = [true, {news_valid}, 200, {news_status}]
raise "Rails: #{{got.inspect}}" unless got == want
puts "PASS""##,
            news_status = if news_routed { 200 } else { 404 },
        );
        let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("PASS"),
            "{initializer}: activerecord/actionpack 8.1.4 disagree\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
        );
    }
}

/// Boot files beyond one initializer: a file `config/application.rb`
/// requires (which runs before any initializer), and an alias written in
/// one initializer and mutated through in another.
fn boot_app(files: &[(&str, &str)], route: bool) -> emit_and_run::Overlay {
    files.iter().fold(app(None, route), |app, (path, source)| app.write(path, source))
}

fn assert_refused(files: &[(&str, &str)]) {
    let (emitted, errors) = boot_app(files, false).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).expect("widget.rb");
    assert!(!widget.contains("[\"news\", \"blog\"]"), "{files:?}\n{widget}");
    let refused = std::panic::catch_unwind(|| boot_app(files, true).ingest_warnings());
    let message = refused.expect_err("the constraint is refused").downcast::<String>().map(|m| *m).unwrap_or_default();
    assert!(message.contains("constraints: id"), "{files:?}: {message}");
}

#[test]
fn a_file_the_application_requires_is_a_boot_source() {
    assert_refused(&[
        ("config/catalog_extensions.rb", "Catalog::KINDS << \"wiki\"\n"),
        ("config/application.rb", "require_relative \"../app/lib/catalog\"\nrequire_relative \"catalog_extensions\"\n"),
    ]);
}

#[test]
fn an_alias_one_initializer_writes_and_another_mutates() {
    assert_refused(&[
        ("config/initializers/a_alias.rb", "KINDS_ALIAS = Catalog::KINDS\n"),
        ("config/initializers/b_extend.rb", "KINDS_ALIAS << \"wiki\"\n"),
    ]);
}

/// A boot directory that can't be read is an error, as a models directory
/// is, rather than a boot source silently left out. Here a nested
/// directory under `config/environments` can't be listed: swallowing that
/// dropped the readable `test.rb` beside it, mutation and all, and the
/// stale list was folded.
#[cfg(unix)]
#[test]
fn an_unreadable_boot_directory_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("roundhouse-boot-unreadable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let routes = "Rails.application.routes.draw do\nend\n";
    for (path, body) in [
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/lib/catalog.rb", CATALOG),
        ("app/models/widget.rb", WIDGET),
        ("config/routes.rb", routes),
        ("config/environments/test.rb", "Catalog::KINDS << \"wiki\"\n"),
        ("config/environments/private/keep.rb", "# placeholder\n"),
    ] {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let private = root.join("app_src/config/environments/private");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A process that can read past the mode (CAP_DAC_OVERRIDE, root) has
    // no unreadable directory to report.
    if std::fs::read_dir(&private).is_ok() {
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&root).ok();
        eprintln!("skipping: this process can list a mode-000 directory");
        return;
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "-o"])
        .arg(root.join("out"))
        .arg(root.join("app_src"))
        .output()
        .expect("run roundhouse");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let widget = std::fs::read_to_string(root.join("out/app/models/widget.rb")).unwrap_or_default();
    std::fs::remove_dir_all(&root).ok();
    assert!(!out.status.success(), "the unreadable directory is reported\n{stderr}\n{widget}");
    assert!(!widget.contains("[\"news\", \"blog\"]"), "{widget}");
}

#[test]
fn a_file_an_initializer_requires_is_a_boot_source() {
    assert_refused(&[
        ("config/catalog_extensions.rb", "Catalog::KINDS << \"wiki\"\n"),
        ("config/initializers/catalog.rb", "require_relative \"../catalog_extensions\"\n"),
    ]);
}

#[test]
fn a_file_boot_rb_requires_is_a_boot_source() {
    assert_refused(&[
        ("config/catalog_extensions.rb", "Catalog::KINDS << \"wiki\"\n"),
        ("config/boot.rb", "require_relative \"catalog_extensions\"\n"),
    ]);
}

/// A namespace that happens to share the constant's name does not hide
/// a mutation of the constant.
#[test]
fn a_namespace_named_like_the_constant_keeps_the_taint() {
    assert_refused(&[
        ("app/lib/kinds.rb", "module KINDS\nend\n"),
        ("config/initializers/catalog.rb", "Catalog::KINDS << \"wiki\"\n"),
    ]);
}

/// A chain of requires runs at boot to its end: an initializer requires
/// `a.rb`, which requires `b.rb`, which mutates the list.
#[test]
fn a_require_chain_is_followed_to_its_end() {
    assert_refused(&[
        ("config/b.rb", "Catalog::KINDS << \"wiki\"\n"),
        ("config/a.rb", "require_relative \"b\"\n"),
        ("config/initializers/catalog.rb", "require_relative \"../a\"\n"),
    ]);
}

/// Two files that require each other end the walk.
#[test]
fn a_require_cycle_ends() {
    assert_refused(&[
        ("config/b.rb", "require_relative \"a\"\nCatalog::KINDS << \"wiki\"\n"),
        ("config/a.rb", "require_relative \"b\"\n"),
        ("config/initializers/catalog.rb", "require_relative \"../a\"\n"),
    ]);
}
