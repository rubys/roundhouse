//! An app class named `Error` shadows the `Error` every Ruby exception
//! maps to on TypeScript (`class Error extends Error`); it is reported
//! there instead of emitted broken.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

#[test]
fn an_app_class_named_error_is_unsupported_on_typescript() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/models/error.rb", "class Error < StandardError\nend\n")
        .emit(BuildTarget::Typescript);
    assert!(errors.iter().any(|e| e.contains("an app class named `Error` shadows")), "{errors:#?}");
}
