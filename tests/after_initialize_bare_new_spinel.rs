//! The Spinel side of `tests/after_initialize_bare_new.rs`: a bare
//! `new` runs `after_initialize` once and a loaded record runs it once,
//! on the native binary as on CRuby. Needs a Spinel compiler, so
//! `#[ignore]`d like its sibling suites; CI's `spinel-framework` job
//! runs it with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_bare_new_runs_after_initialize_once_on_spinel() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  after_initialize do\n    self.name = \"#{name}!\"\n  end\nend\n",
        )
        .run_spinel_with_db(r#"
bare = Widget.new.name
raise "bare new: #{bare.inspect}" unless bare == "!"
with_attrs = Widget.new(name: "x").name
raise "new(attrs): #{with_attrs.inspect}" unless with_attrs == "x!"
w = Widget.create!(name: "Hooked")
raise "create!: #{w.name.inspect}" unless w.name == "Hooked!"
found = Widget.find(w.id).name
raise "find: #{found.inspect}" unless found == "Hooked!!"
puts "after_initialize once"
"#)
        .assert_passes();
}
