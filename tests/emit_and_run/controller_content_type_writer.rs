use super::emit_and_run;
use std::path::Path;
use std::process::Command;

#[test]
fn controller_writer_values_and_headers_match_rails() {
    let support = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/controller_content_type_writer");
    let available = Command::new("ruby").arg("-e")
        .arg("require './tests/support/rails_oracle'; exit(RailsOracle.available?('actionpack') ? 0 : 1)")
        .status().expect("probe Rails gems");
    if !available.success() {
        eprintln!("skipping controller content-type oracle: actionpack oracle gems are unavailable");
        return;
    }
    let native = Command::new("ruby").arg(support.join("oracle.rb")).arg("rails")
        .output().expect("run Rails oracle");
    assert!(native.status.success(), "Rails oracle failed: {}", String::from_utf8_lossy(&native.stderr));
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/controllers/concerns/media_headers.rb", include_str!("../support/controller_content_type_writer/concern.rb"))
        .write("app/controllers/media_controller.rb", include_str!("../support/controller_content_type_writer/controller.rb"))
        .write("app/controllers/other_media_controller.rb", include_str!("../support/controller_content_type_writer/other_controller.rb"))
        .write("app/controllers/custom_media_controller.rb", include_str!("../support/controller_content_type_writer/custom_controller.rb"))
        .write("config/routes.rb", include_str!("../support/controller_content_type_writer/routes.rb"))
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let actual = Command::new("ruby").arg(support.join("oracle.rb")).arg("emitted")
        .env("BLOG_DB", ":memory:").current_dir(&*emitted).output().expect("run emitted oracle");
    assert!(actual.status.success(), "emitted oracle failed: {}", String::from_utf8_lossy(&actual.stderr));
    let expected: serde_json::Value = serde_json::from_slice(&native.stdout).expect("Rails JSON");
    let actual: serde_json::Value = serde_json::from_slice(&actual.stdout).expect("emitted JSON");
    assert_eq!(actual, expected);
}
