use super::emit_and_run;

/// An initializer's top-level `def` is a method on Object (lobsters'
/// `do_heinous_inline_partial_replacement`, called from a development-only
/// filter). It becomes a class method of the synthesized
/// `RoundhouseInitializers`, and the call names that module, so it is
/// defined wherever it is called (`ingest::boot_program`).
#[test]
fn a_call_to_an_initializer_top_level_def_reaches_the_boot_module() {
    let app = emit_and_run::real_blog()
        .write("config/initializers/stamp.rb", "def stamp_request\n  Rails.logger.info(\"stamped\")\nend\n")
        .edit(
            "app/controllers/application_controller.rb",
            "  stale_when_importmap_changes\n",
            "  stale_when_importmap_changes\n\n  before_action :stamp\n\n  def stamp\n    stamp_request\n  end\n",
        );
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/application_controller.rb")).unwrap();
    assert!(controller.contains("RoundhouseInitializers.stamp_request"), "{controller}");
}
