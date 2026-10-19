//! `if defined?(Rails::Console) … def info … end` declares a method only
//! under `rails console`, never in the app a target emits (a server or
//! its tests), so the branch declares nothing and the module around it
//! is carried. discourse's `SiteSettingExtension` was dropped whole for
//! its one console-only helper.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "lib/greeting.rb",
            "module Greeting\n  def self.hello(name)\n    \"hello #{name}\"\n  end\n\n  if defined?(Rails::Console)\n    def self.info(name)\n      name\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.greet\n    Greeting.hello(\"blog\")\n  end\n",
        )
}

const ASSERTIONS: &str = r##"raise Article.greet.inspect unless Article.greet == "hello blog"
raise "console" if Greeting.respond_to?(:info)
puts "console only passed"
"##;

#[test]
fn a_console_only_definition_leaves_its_module_carried() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn a_console_only_definition_is_not_emitted() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let greeting = std::fs::read_to_string(emitted.join("app/models/greeting.rb")).expect("greeting emitted");
    assert!(greeting.contains("def self.hello"), "{greeting}");
    assert!(!greeting.contains("info"), "{greeting}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_console_only_definition_leaves_its_module_carried_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}
