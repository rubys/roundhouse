//! Regression test for roundhouse#296.
//!
//! `save!`/`destroy`/`destroy!`/`reload` are declared `() -> Base` in
//! `runtime/ruby/active_record/base.rbs`, literally — not a self-type
//! (RBS self-types the parser doesn't read yet, per that file's own
//! comment). Unlike `find`/`create!`, these four are never
//! monomorphized per model at emit time: every model shares the one
//! compiled `Base` method, which returns a `Base`-typed value no
//! matter which subclass calls it.
//!
//! Before the fix, the catalog seeded all four as `ReturnKind::SelfType`
//! (the receiver's own class), so a plain method whose tail calls one
//! of them — `WidgetStamp#run` below, lifted from the issue — got an
//! emitted `.rbs` claiming `-> Widget`. Spinel's AOT build refused that:
//! the generated C returns `sp_ActiveRecord__Base *` from a function
//! typed `sp_Widget *`. This test pins the emitted shape (no spinel
//! binary required — see `tests/testing.md`'s ".rbs-assertion" lane),
//! so it fails on main for the same reason the real build does: a
//! false claim in the emitted signature, not merely a missing feature.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn widget_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def index
    widget = Widget.create!(name: "a")
    result = WidgetStamp.new.run(widget.id)
    render plain: "#{result.inspect} #{Widget.find(widget.id).name}"
  end
end
"##,
        )
}

/// `record.save!` as a plain method's tail — the issue's exact shape.
#[test]
fn save_bang_tail_seeds_the_runtime_base_type_not_the_record_class() {
    let app = widget_app().write(
        "app/models/widget_stamp.rb",
        "class WidgetStamp\n  def run(id)\n    widget = Widget.find(id)\n    widget.name = \"stamped\"\n    widget.save!\n  end\nend\n",
    );
    let (emitted, errors) = app.emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emit reported errors: {errors:?}");
    let rbs = std::fs::read_to_string(emitted.join("app/models/widget_stamp.rbs"))
        .expect("read emitted widget_stamp.rbs");
    assert!(
        !rbs.contains("-> Widget"),
        "`run` must not claim the record's own class — the compiled \
         `save!` is a shared Base method, not monomorphized per model:\n{rbs}"
    );
    assert!(
        rbs.contains("def run: (Integer id) -> ActiveRecord::Base"),
        "`run` should be seeded with what `save!` actually returns \
         (`ActiveRecord::Base`, matching the sidecar literally):\n{rbs}"
    );
}

/// `destroy`, `destroy!` and `reload` are declared `() -> Base` the
/// same way `save!` is (same file, same reasoning) — same bug, same fix.
#[test]
fn destroy_destroy_bang_and_reload_tails_also_seed_the_runtime_base_type() {
    for (method, name) in [
        ("destroy", "widget_destroy"),
        ("destroy!", "widget_destroy_bang"),
        ("reload", "widget_reload"),
    ] {
        let app = widget_app().write(
            "app/models/widget_stamp.rb",
            &format!(
                "class WidgetStamp\n  def run(id)\n    widget = Widget.find(id)\n    widget.{method}\n  end\nend\n"
            ),
        );
        let (emitted, errors) = app.emit(BuildTarget::Spinel);
        assert!(errors.is_empty(), "[{name}] analysis/emit reported errors: {errors:?}");
        let rbs = std::fs::read_to_string(emitted.join("app/models/widget_stamp.rbs"))
            .unwrap_or_else(|e| panic!("[{name}] read emitted widget_stamp.rbs: {e}"));
        assert!(
            !rbs.contains("-> Widget"),
            "[{name}] `run` must not claim the record's own class:\n{rbs}"
        );
        assert!(
            rbs.contains("-> ActiveRecord::Base"),
            "[{name}] `run` should be seeded with `ActiveRecord::Base`:\n{rbs}"
        );
    }
}
