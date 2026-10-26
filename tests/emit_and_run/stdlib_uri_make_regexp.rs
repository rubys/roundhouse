use super::emit_and_run;

/// `format: { with: URI::DEFAULT_PARSER.make_regexp }`: a method of the
/// standard library that builds a Regexp, not a constant. Its value is a
/// 1440-character extended-mode (`/x`) pattern with comments and a
/// `(?!//)` lookahead, which no strict target's regex engine takes as
/// is. It is unanchored, though, and everything after the scheme's colon
/// is optional (a `//` after it matches the empty authority), so
/// `match?` on it is true exactly when the string holds
/// `[a-zA-Z][a-zA-Z0-9+.-]*:`. That is the pattern the rule gets.
///
/// Answers are Rails 8.1.4's (Ruby 4.0, uri 1.1) for the same model and
/// values. The two `with:` spellings are the same Regexp there
/// (`RFC2396_PARSER.make_regexp == DEFAULT_PARSER.make_regexp`).
const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :link, format: { with: URI::DEFAULT_PARSER.make_regexp }
  validates :site, presence: true, format: { with: ::URI::RFC2396_PARSER.make_regexp }, allow_nil: true
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"link\"\n    t.string \"site\"\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

#[test]
fn uri_make_regexp_reports_no_gap() {
    let warnings = app().ingest_warnings();
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn uri_make_regexp_validates_like_rails() {
    app()
        .run_ruby(
            r#"values = ["https://example.com/a?b#c", "mailto:a@b", "not a url", "", nil, "1:", "see x:y", "ftp:"]
got = values.map do |v|
  w = Widget.new(link: v, site: v)
  w.valid?
  w.errors.to_a
end
want = [
  [],
  [],
  ["Link is invalid", "Site is invalid"],
  ["Link is invalid", "Site can't be blank", "Site is invalid"],
  ["Link is invalid"],
  ["Link is invalid", "Site is invalid"],
  [],
  [],
]
raise "errors: #{got.inspect}" unless got == want
puts "uri make_regexp passed"
"#,
        )
        .assert_passes();
}

#[test]
fn uri_make_regexp_emits_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}

/// `make_regexp(%w[http https])` builds a different pattern (a scheme
/// lookahead), and an app that defines its own top-level `URI` may have
/// replaced the parser: both stay the reported gap.
#[test]
fn schemes_or_an_app_defined_uri_keep_the_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  validates :link, format: { with: URI::DEFAULT_PARSER.make_regexp(%w[http https]) }\nend\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("validates :link, format: with:")), "{warnings:?}");

    let warnings = app().write("lib/uri_stub.rb", "URI = Object.new\n").ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("validates :link, format: with:")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("validates :site, format: with:")), "{warnings:?}");
}
