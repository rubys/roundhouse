//! A model that `extend`s an app module and calls its methods in the
//! class body (discourse's `SiteSetting`: `extend SiteSettingExtension`,
//! `load_settings(…)`, which calls `define_singleton_method(name)` for
//! each key of a YAML file) runs that code as it loads, on the Ruby
//! family as in Rails, among its own methods in source order. A call
//! the source does not declare then answers untyped there: the code
//! defines it. A call the replay cannot carry (inside a block) keeps the
//! class closed. A compiled target reports the replayed statements and
//! the computed definition.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "lib/settings_extension.rb",
            "module SettingsExtension\n  def setting(name, default)\n    define_singleton_method(name) { default }\n  end\n\n  if defined?(Rails::Console)\n    def info(name)\n      name\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n  extend SettingsExtension\n  setting :title_limit, 40\n\n  def self.limit\n    Article.title_limit\n  end\n",
        )
}

#[test]
fn a_computed_name_reads_untyped_and_runs() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app()
        .run_ruby("raise Article.limit.inspect unless Article.limit == 40\nraise \"console\" if SettingsExtension.method_defined?(:info)\nputs \"open class passed\"\n")
        .assert_passes();
}

#[test]
fn a_computed_name_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = app().emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("`define_singleton_method` with a name computed when it runs")), "{errors:#?}");
}

/// Ruby order: a method defined after the call overrides what it defines.
#[test]
fn a_def_after_the_call_overrides_it() {
    app()
        .edit("app/models/article.rb", "  def self.limit\n", "  def self.title_limit\n    99\n  end\n\n  def self.limit\n")
        .run_ruby("raise Article.limit.inspect unless Article.limit == 99\nputs \"order passed\"\n")
        .assert_passes();
}

/// A call inside a block is not replayed, so nothing defines the name.
#[test]
fn a_call_the_replay_cannot_carry_keeps_the_class_closed() {
    let errors = app()
        .edit("app/models/article.rb", "  setting :title_limit, 40\n", "  %w[title_limit].each { |n| setting(n, 40) }\n")
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("title_limit")), "{errors:#?}");
}

/// A module opened in two files keeps both openings in the Ruby emit, so the macro a reopening supplies is replayed and runs.
#[test]
fn a_reopened_module_s_macro_runs() {
    emit_and_run::real_blog()
        .write("lib/settings_extension.rb", "module SettingsExtension\n  def settings_loaded?\n    true\n  end\nend\n")
        .write("lib/settings_extension/setting.rb", "module SettingsExtension\n  def setting(name, default)\n    define_singleton_method(name) { default }\n  end\nend\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n  extend SettingsExtension\n  setting :title_limit, 40\n\n  def self.limit\n    Article.title_limit\n  end\n",
        )
        .run_ruby("raise Article.limit.inspect unless Article.limit == 40\nraise \"loaded\" unless Article.settings_loaded?\nputs \"reopened passed\"\n")
        .assert_passes();
}
