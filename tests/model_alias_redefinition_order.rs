//! `lower::model_to_library::resolve_model_aliases` spliced its
//! resolved alias copy into `methods` by name, unconditionally
//! overwriting whatever was already at that slot — including a
//! hand-written `def new` declared AFTER the alias in the model body.
//! Ruby's own class body runs top to bottom, so the LAST definition of
//! a name wins:
//!
//!   - `def new` AFTER `alias new old` → the later `def` is the last
//!     definition and must survive;
//!   - `def new` BEFORE `alias new old` → the alias is the last
//!     definition and must replace it.
//!
//! The resolver got the second case right (nothing to protect) but
//! silently lost the first, splicing the alias copy over a `def` that
//! should have won. Checked under real CRuby (`emit_and_run`), not
//! just by inspecting the lowered `MethodDef`, since the whole point
//! is that the EMITTED program's behavior must match Ruby's.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn widget_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        )
}

#[test]
fn a_def_after_the_alias_is_the_last_definition_and_survives() {
    let run = widget_app()
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  def greeting\n    \"original\"\n  end\n  alias greet greeting\n  def greet\n    \"overridden\"\n  end\nend\n",
        )
        .run_ruby(
            r#"
widget = Widget.new
raise "later def must win over the earlier alias, got #{widget.greet.inspect}" unless widget.greet == "overridden"
raise "greeting itself must be untouched" unless widget.greeting == "original"
"#,
        );
    run.assert_passes();
}

#[test]
fn a_def_before_the_alias_is_replaced_by_the_alias_copy() {
    let run = widget_app()
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  def greet\n    \"stale\"\n  end\n  def greeting\n    \"original\"\n  end\n  alias greet greeting\nend\n",
        )
        .run_ruby(
            r#"
widget = Widget.new
raise "the alias, as the LAST definition, must win, got #{widget.greet.inspect}" unless widget.greet == "original"
raise "greet must answer the same as greeting" unless widget.greet == widget.greeting
"#,
        );
    run.assert_passes();
}
