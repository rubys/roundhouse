//! The Spinel side of a block-form `around_action` (`ingest::controller::
//! around_block_filter`, rubys/roundhouse#778): the synthesized method
//! must compile and run the same way the CRuby check in
//! `tests/around_block_filters.rs` does. Needs a Spinel compiler, so
//! `#[ignore]`d like its sibling suites; CI's `spinel-framework` job
//! runs it with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn gadgets() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gadgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/boom\", to: \"gadgets#boom\"\n  get \"/calm\", to: \"gadgets#calm\"\nend\n",
        )
        .write(
            "app/controllers/gadgets_controller.rb",
            r#"class GadgetsController < ApplicationController
  around_action(only: [:boom, :calm]) do |_controller, block|
    block.call
  ensure
    response.headers['X-Quota'] = '9'
  end

  def boom
    raise "kaboom"
  end

  def calm
    head :ok
  end
end
"#,
        )
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_block_form_around_action_compiles_and_runs_on_spinel() {
    let run = gadgets().run_spinel(
        r##"
calm = GadgetsController.new
calm.process_action(:calm)
raise "the action itself must still run" unless calm.headers["X-Quota"] == "9"

controller = GadgetsController.new
raised = false
begin
  controller.process_action(:boom)
rescue RuntimeError => e
  raised = (e.message == "kaboom")
end
raise "the action's raise must propagate" unless raised
raise "ensure must still set the header" unless controller.headers["X-Quota"] == "9"
puts "around ensure ran on both the calm and the raising path (spinel)"
"##,
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("around ensure ran on both the calm and the raising path (spinel)"),
        "{}",
        run.stdout
    );
}
