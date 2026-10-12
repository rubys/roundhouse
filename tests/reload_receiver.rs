//! `reload` and `lock!` answer their receiver, so model-defined methods
//! still resolve on the result.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const WIDGET: &str = "class Widget < ApplicationRecord
  def label
    name
  end
  def self.refreshed_label(id)
    find(id).reload.label
  end
  def self.locked_label(id)
    find(id).lock!.label
  end
end
";

const ASSERTIONS: &str = r#"
widget = Widget.create!(name: 'fresh')
raise unless Widget.refreshed_label(widget.id) == 'fresh'
raise unless Widget.locked_label(widget.id) == 'fresh'
stamp = WidgetStamp.new
raise unless stamp.refreshed(widget.id).label == 'fresh'
raise unless stamp.locked(widget.id).label == 'fresh'
Db.exec("UPDATE widgets SET name = 'reloaded' WHERE id = #{widget.id}")
raise unless widget.reload.equal?(widget) && widget.label == 'reloaded'
Db.exec("UPDATE widgets SET name = 'locked' WHERE id = #{widget.id}")
raise unless widget.lock!.equal?(widget) && widget.label == 'locked'
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", WIDGET)
        // A tail call also emits a model return signature. Exercise it
        // natively: the inherited runtime methods are declared on Base.
        .write("app/models/widget_stamp.rb", "class WidgetStamp\n  def refreshed(id)\n    Widget.find(id).reload\n  end\n  def locked(id)\n    Widget.find(id).lock!\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn reload_and_lock_keep_the_receiver_type_and_run() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn reload_forwards_optional_options_to_the_runtime() {
    example().run_ruby(r#"
observed = []
ActiveRecord::Base.prepend(Module.new do
  define_method(:reload) do |options = nil|
    observed << options
    super(options)
  end
end)
widget = Widget.create!(name: 'fresh')
raise unless widget.reload.equal?(widget)
raise unless widget.reload(nil).equal?(widget)
raise unless widget.reload({ lock: true }).equal?(widget)
raise "reload options not forwarded: #{observed.inspect}" unless observed == [nil, nil, { lock: true }]
"#).assert_passes();
}

#[test]
fn bare_lock_forwards_true_to_the_runtime() {
    example().run_ruby(r#"
observed = []
# Observe after emission so this runtime probe does not suppress the forwarder.
ActiveRecord::Base.prepend(Module.new do
  define_method(:lock!) do |lock = true|
    observed << lock
    super(lock)
  end
end)
widget = Widget.create!(name: 'locked')
raise unless widget.lock!.equal?(widget)
raise 'bare lock! must pass true to the runtime' unless observed == [true]
for lock in [false, nil, 'FOR UPDATE NOWAIT']
  raise unless widget.lock!(lock).equal?(widget)
end
raise unless observed == [true, false, nil, 'FOR UPDATE NOWAIT']
"#).assert_passes();
}

#[test]
fn reload_and_lock_sidecars_keep_the_receiver_type() {
    let (emitted, errors) = example().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rbs")).unwrap();
    assert!(
        widget.lines().any(|line| {
            line.contains("def reload:") && line.contains("-> Widget")
        }),
        "{widget}",
    );
    assert!(widget.lines().any(|line| line.contains("def lock!:") && line.ends_with("-> Widget")), "{widget}");
    let stamp = std::fs::read_to_string(emitted.join("app/models/widget_stamp.rbs")).unwrap();
    for method in ["refreshed", "locked"] {
        assert!(stamp.lines().any(|line| {
            line.trim_start().starts_with(&format!("def {method}:")) && line.ends_with("-> Widget")
        }), "{stamp}");
    }
}

#[test]
fn source_reload_override_is_not_wrapped() {
    example()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n  def reload\n    Widget.create!(name: 'override')\n  end\nend\n")
        .run_ruby("widget = Widget.create!(name: 'original')\nraise if widget.reload.equal?(widget)\nraise unless Widget.refreshed_label(widget.id) == 'override'\nraise unless Widget.locked_label(widget.id) == 'override'\n")
        .assert_passes();
}

#[test]
fn namespaced_parent_reload_override_is_not_wrapped() {
    // `< Base` inside `module Admin` writes the parent as `Base`; the
    // emitter must resolve it to `Admin::Base` before deciding whether
    // to interpose a self-returning forwarder.
    let (emitted, errors) = example()
        .write(
            "app/models/admin/base.rb",
            "module Admin\n  class Base < ApplicationRecord\n    primary_abstract_class\n    def reload\n      ::Widget.create!(name: 'parent override')\n    end\n  end\nend\n",
        )
        .write(
            "app/models/admin/widget.rb",
            "module Admin\n  class Widget < Base\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let src = std::fs::read_to_string(emitted.join("app/models/admin/widget.rb")).unwrap();
    assert!(
        !src.contains("def reload"),
        "parent override was discarded by an unresolved Base walk:\n{src}"
    );
    let output = emit_and_run::ruby()
        .args(["-e", r#"
require File.expand_path('main', Dir.pwd)
Main.configure_default_adapter!
widget = Admin::Widget.create!(name: 'original')
reloaded = widget.reload
raise 'parent reload result was discarded' unless reloaded.name == 'parent override'
raise if reloaded.equal?(widget)
"#])
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn source_reload_override_return_is_not_seeded_as_self() {
    // Analyzer must not seed Widget#reload as Widget when ApplicationRecord
    // overrides it to return another type — BodyTyper would prefer the seed.
    let (emitted, errors) = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name\n  end\n  create_table :others do |t|\n    t.string :name\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n  def reload\n    Other.create!(name: 'override')\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/models/other.rb",
            "class Other < ApplicationRecord\n  def tag\n    'other'\n  end\nend\n",
        )
        .write(
            "app/models/probe.rb",
            "class Probe\n  def from_reload(id)\n    Widget.find(id).reload.tag\n  end\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let probe = ["sig/app/models/probe.rbs", "app/models/probe.rbs"]
        .iter()
        .find_map(|path| std::fs::read_to_string(emitted.join(path)).ok())
        .expect("probe.rbs");
    // If reload were seeded as Widget, `.tag` would not resolve (Widget has
    // no `tag`) and the probe signature would stay untyped/error.
    assert!(
        probe.lines().any(|line| {
            line.trim_start().starts_with("def from_reload:") && line.contains("-> String")
        }),
        "reload override return was hidden by a self seed:\n{probe}"
    );
}

#[test]
fn relative_concern_reload_override_is_not_wrapped() {
    let source = r#"
module Admin
  class Widget < ApplicationRecord
    include Concerns::Reloading
  end
end
"#;
    let (emitted, mut app, errors) = example()
        .write("app/models/admin/concerns/reloading.rb", r#"
module Admin
  module Concerns
    module Reloading
      def reload
        ::Widget.create!(name: 'concern override')
      end
    end
  end
end
"#)
        .write("app/models/admin/widget.rb", source)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    // Whole-app ingest qualifies includes. The per-file API retains the
    // written path: emit that model too, without relying on normalization.
    let model = roundhouse::ingest::ingest_model(
        source.as_bytes(), "app/models/admin/widget.rb", &app.schema, &Default::default(),
    ).unwrap().unwrap();
    let destination = app.models.iter_mut().find(|m| m.name == model.name).unwrap();
    *destination = model;
    for file in roundhouse::emit::ruby::emit_lowered_models(&app) {
        std::fs::write(emitted.join(file.path), file.content).unwrap();
    }
    let output = emit_and_run::ruby()
        .args(["-e", r#"
require File.expand_path('main', Dir.pwd)
Main.configure_default_adapter!
widget = Admin::Widget.create!(name: 'original')
reloaded = widget.reload
raise 'concern reload result was discarded' unless reloaded.name == 'concern override'
raise if reloaded.equal?(widget)
raise 'concern lock result was discarded' unless widget.lock!.name == 'concern override'
"#])
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn reload_and_lock_keep_the_receiver_type_natively() {
    let script = format!("Db.configure(\":memory:\")\nDb.exec(\"CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT)\")\n{ASSERTIONS}");
    example().run_spinel_with_rbs(&script).assert_passes();
}
