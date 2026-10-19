//! `Regexp.new(pattern, Regexp::IGNORECASE, timeout: 0.1)` and
//! `rescue RegexpError`, as lobsters' domain selectors validate a pattern
//! a moderator types in.

use super::emit_and_run;

fn regexp_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def self.selector(pattern)\n    Regexp.new(pattern, Regexp::IGNORECASE, timeout: 0.1)\n  end\n\n  def self.check(pattern)\n    selector(pattern)\n    \"ok\"\n  rescue RegexpError => e\n    \"bad: #{e.class}\"\n  end\n\n  def self.flags\n    [Regexp::IGNORECASE, Regexp::EXTENDED, Regexp::MULTILINE]\n  end\n",
    )
}

const ASSERTIONS: &str = r##"got = [Article.selector("\\Aab\\z").match?("AB"), Article.check("("), Article.check("a+"), Article.flags]
raise got.inspect unless got == [true, "bad: RegexpError", "ok", [1, 2, 4]]
puts "regexp constants passed"
"##;

#[test]
fn regexp_option_constants_and_regexp_error_run() {
    regexp_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn regexp_option_constants_and_regexp_error_run_on_spinel() {
    regexp_app().run_spinel(ASSERTIONS).assert_passes();
}

/// No strict target has Ruby's Regexp to read the option bits from, nor
/// a `RegexpError` to rescue: each is reported there.
#[test]
fn regexp_option_constants_are_unsupported_on_strict_targets() {
    let (_emitted, errors) = regexp_app().emit(roundhouse::project::BuildTarget::Rust);
    for name in ["Regexp::IGNORECASE is Ruby's option bit", "Regexp::MULTILINE is Ruby's option bit", "RegexpError is not available"] {
        assert!(errors.iter().any(|e| e.contains(name)), "{name}: {errors:#?}");
    }
}

/// `Regexp.new` / `.escape` / `.union` name Ruby's class, which no strict
/// emit maps (each emitted the call against a name nothing defines);
/// they are reported there. A regexp literal is not: each target maps it.
#[test]
fn regexp_class_methods_are_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = emit_and_run::real_blog()
            .edit(
                "app/models/article.rb",
                "  validates :title, presence: true\n",
                "  validates :title, presence: true\n\n  def self.built(s)\n    Regexp.new(s).match?(\"abc\")\n  end\n\n  def self.escaped(s)\n    Regexp.escape(s)\n  end\n\n  def self.literal(s)\n    /ab/.match?(s)\n  end\n",
            )
            .emit(target);
        for method in ["new", "escape"] {
            assert!(errors.iter().any(|e| e.contains(&format!("`Regexp.{method}` is Ruby's Regexp class"))), "{target:?} {method}: {errors:#?}");
        }
        assert_eq!(errors.len(), 2, "{target:?}: {errors:#?}");
    }
}
