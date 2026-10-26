//! A class method an app writes to define methods (forem's
//! `Settings::Base.setting`, adapted from rails-settings-cached) is
//! expanded at each class-body call into the methods it defines. Its
//! arguments are captured as Ruby captures them: a literal is that
//! literal, any other value is evaluated once, where the call is.

use super::emit_and_run;

const BASE: &str = r###"class SettingsBase < ApplicationRecord
  self.abstract_class = true

  PROTECTED_KEYS = %w[var value].freeze

  class << self
    def setting(key, default: nil, type: :string)
      define_setting(key, default: default, type: type)
    end

    def keys
      @defined_settings.map { |s| s[:key] }
    end

    private

    def define_setting(key, default: nil, type: :string)
      key = key.to_s
      raise(ArgumentError, "Can't use '#{key}' as setting name") if key.in?(PROTECTED_KEYS)

      @defined_settings ||= []
      @defined_settings << { key: key, default: default, type: type }

      define_singleton_method(key) do |scope: nil|
        [1].each { |key| key.to_s }
        result = stored(key)
        result = default if result.nil?
        convert(type, result)
      end

      define_singleton_method(:"#{key}=") do |value|
        store(key, value)
        value
      end
    end

    def stored(key)
      (@values ||= {})[key]
    end

    def store(key, value)
      (@values ||= {})[key] = value.to_s
    end

    def convert(type, value)
      case type
      when :integer then value.to_i
      else value.to_s
      end
    end
  end
end
"###;

const SITE: &str = r#"class SiteConfig < SettingsBase
  self.table_name = :site_configs

  setting :community_name, type: :string, default: "Forem"
  setting :start_year, type: :integer, default: 2000 + 26
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/settings_base.rb", BASE)
        .write("app/models/site_config.rb", SITE)
        .edit(
            "db/schema.rb",
            "  add_foreign_key",
            "  create_table \"site_configs\", force: :cascade do |t|\n    t.string \"var\"\n    t.text \"value\"\n  end\n\n  add_foreign_key",
        )
}

const ASSERTIONS: &str = r##"got = [SiteConfig.community_name, SiteConfig.start_year.to_s]
SiteConfig.community_name = "Dev"
SiteConfig.start_year = 2030
got = got + [SiteConfig.community_name, SiteConfig.start_year.to_s] + SiteConfig.keys
raise got.inspect unless got == ["Forem", "2026", "Dev", "2030", "community_name", "start_year"]
puts "class macros passed"
"##;

#[test]
fn a_setting_macro_defines_its_methods() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_setting_macro_defines_its_methods_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

/// A guard the call's arguments do not decide leaves the call as it was,
/// reported, rather than dropping the methods it would have defined.
#[test]
fn a_macro_it_cannot_run_is_reported() {
    let (_emitted, errors) = app()
        .edit("app/models/site_config.rb", "  setting :start_year", "  setting :nickname, type: (Time.now.year > 2000 ? :string : :integer)\n  setting :start_year")
        .edit("app/models/settings_base.rb", "      key = key.to_s\n", "      key = key.to_s\n      raise ArgumentError, \"bad type\" unless type == :string || type == :integer\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`setting` defines methods this call does not expand")), "{errors:#?}");
}

/// The expansion runs the macro's writes to the class's own state in the
/// calling class's body, which no strict target carries.
#[test]
fn a_class_body_state_write_is_unsupported_on_strict_targets() {
    let refused = std::panic::catch_unwind(|| { app().emit(roundhouse::project::BuildTarget::Rust); }).expect_err("the Rust emit refuses");
    let message = refused.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("class-instance-variable initialization is not supported"), "{message}");
}

/// A rescue list with a splat (`rescue ArgumentError, *ERRORS => e`) in a
/// definition's block is not an argument splat: the IR keeps it as written
/// (a `Splat` among the rescue clause's classes), so the expansion keeps
/// the definition instead of refusing the macro (#1044).
#[test]
fn a_definition_rescuing_a_splatted_error_list_is_expanded() {
    let (_emitted, errors) = app()
        .edit(
            "app/models/settings_base.rb",
            "    def keys\n",
            "    def guarded(name)\n      define_singleton_method(name) do |hash, key|\n        begin\n          hash.fetch(key)\n        rescue ArgumentError, *LOOKUP_ERRORS => e\n          \"rescued #{e.class}\"\n        end\n      end\n    end\n\n    def keys\n",
        )
        .edit("app/models/settings_base.rb", "  PROTECTED_KEYS", "  LOOKUP_ERRORS = [KeyError, IndexError].freeze\n\n  PROTECTED_KEYS")
        .edit("app/models/site_config.rb", "  setting :community_name", "  guarded :lookup\n  setting :community_name")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("defines methods this call does not expand")), "{errors:#?}");
}

#[test]
fn a_definition_rescuing_a_splatted_error_list_runs() {
    app()
        .edit(
            "app/models/settings_base.rb",
            "    def keys\n",
            "    def guarded(name)\n      define_singleton_method(name) do |hash, key|\n        begin\n          hash.fetch(key)\n        rescue ArgumentError, *LOOKUP_ERRORS => e\n          \"rescued #{e.class}\"\n        end\n      end\n    end\n\n    def keys\n",
        )
        .edit("app/models/settings_base.rb", "  PROTECTED_KEYS", "  LOOKUP_ERRORS = [KeyError, IndexError].freeze\n\n  PROTECTED_KEYS")
        .edit("app/models/site_config.rb", "  setting :community_name", "  guarded :lookup\n  setting :community_name")
        .run_ruby(
            "got = [SiteConfig.lookup({ \"a\" => 1 }, \"a\").to_s, SiteConfig.lookup({}, \"b\")]\nraise got.inspect unless got == [\"1\", \"rescued KeyError\"]\nputs \"splat rescue passed\"\n",
        )
        .assert_passes();
}

/// Control: an argument splat in the same definition is still one the
/// expansion would erase, so the macro is still refused.
#[test]
fn a_definition_with_an_argument_splat_is_still_refused() {
    let (_emitted, errors) = app()
        .edit(
            "app/models/settings_base.rb",
            "    def keys\n",
            "    def guarded(name)\n      define_singleton_method(name) do |hash, key|\n        begin\n          hash.fetch(*[key])\n        rescue ArgumentError, *LOOKUP_ERRORS => e\n          \"rescued #{e.class}\"\n        end\n      end\n    end\n\n    def keys\n",
        )
        .edit("app/models/settings_base.rb", "  PROTECTED_KEYS", "  LOOKUP_ERRORS = [KeyError, IndexError].freeze\n\n  PROTECTED_KEYS")
        .edit("app/models/site_config.rb", "  setting :community_name", "  guarded :lookup\n  setting :community_name")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`guarded` defines methods this call does not expand")), "{errors:#?}");
}
