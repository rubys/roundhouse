//! A `Representable::Decorator` is expanded into plain methods and
//! renders as JSON without the gem, byte for byte as representable does.

use super::representable_decorator;

/// A `Representable::Decorator` is expanded into plain methods
/// (`ingest::representable`), and `render json: <decorator>.to_hash`
/// answers through its generated `as_json_str` - no representable gem
/// in the emitted tree. The expected text is what representable 3.2.0
/// itself renders for the same object: String keys in declaration
/// order, `as:`, both getter forms, `exec_context: :decorator`, a nested
/// collection, a nil value left out unless `render_nil: true`.
#[test]
fn a_representable_decorator_renders_without_the_gem() {
    representable_decorator::overlay()
        .run_ruby(&format!("{}{}", r#"
require_relative "app/controllers/article_json_controller"
source = File.read("app/controllers/article_json_controller.rb")
raise "the runtime encoder is still reached:\n#{source}" if source.include?("JsonRender")
"#, representable_decorator::ASSERTIONS))
        .assert_passes();
}
