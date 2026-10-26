use super::emit_and_run;

/// #985 follow-up / #1077: `validates :attr, format: { with: … }` with a
/// regex flag — a flagged literal, a flagged model constant, and the
/// Rails idiom of interpolating one flagged constant into another
/// (`ONLY = /\A#{INNER}\z/i`) — used to be an unconditional `format:
/// with` survey gap (`ingest/model/validations.rs`'s `bare_regexp`
/// refused any flag), so the attribute's check silently vanished and
/// Rails-rejected records saved clean. `i` (case-insensitive) and `m`
/// (Ruby's dot-matches-newline) now carry through to every emit
/// target; anything else (here, `x`) stays the gap it always was.
const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend

  NAME_RE = /\A[a-z]+\z/i
  INNER = /[a-z0-9_]+([.-]+[a-z0-9_]+)*/i
  ONLY = /\A#{INNER}\z/i

  validates :title, presence: true
  validates :name_i, format: { with: /\A[a-z0-9_]+\z/i }
  validates :dotall, format: { with: /\Aa.b\z/m }
  validates :no_dotall, format: { with: /\Aa.b\z/ }
  validates :code, format: { with: NAME_RE }
  validates :handle, format: { with: ONLY }
  validates :loose, format: { with: /a/x }
end
"#;

const SCHEMA_FIND: &str =
    "    t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n";
const SCHEMA_REPLACE: &str = "    t.string \"title\"\n    t.text \"body\"\n    t.string \"name_i\"\n    t.string \"dotall\"\n    t.string \"no_dotall\"\n    t.string \"code\"\n    t.string \"handle\"\n    t.string \"loose\"\n    t.datetime \"created_at\", null: false\n";

/// Same flagged validations, minus the `ONLY = /\A#{INNER}\z/i`
/// constant: a class-body constant whose VALUE interpolates another
/// regex desugars (in plain expression ingest, independent of
/// `format:`) to `Regexp.new(…)`, which strict targets already refuse
/// to emit (`regexp_class_methods_are_unsupported_on_strict_targets`
/// in `tests/emit_and_run/regexp_constants.rs`) — a pre-existing,
/// orthogonal gap about interpolated-regex CONSTANT DECLARATIONS, not
/// about `format:` flags (a non-interpolated constant like `NAME_RE`
/// below emits fine everywhere). Widening that gap is out of scope
/// here, so the cross-target sweep below uses this narrower model and
/// the interpolated-constant case is only exercised on Ruby, above.
const MODEL_WITHOUT_INTERPOLATED_CONSTANT: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend

  NAME_RE = /\A[a-z]+\z/i

  validates :title, presence: true
  validates :name_i, format: { with: /\A[a-z0-9_]+\z/i }
  validates :dotall, format: { with: /\Aa.b\z/m }
  validates :no_dotall, format: { with: /\Aa.b\z/ }
  validates :code, format: { with: NAME_RE }
  validates :loose, format: { with: /a/x }
end
"#;

#[test]
fn format_flags_match_rails_regexp_semantics() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write("app/models/article.rb", MODEL)
        .run_ruby(r##"
base = {
  title: "Ok", body: "0123456789",
  name_i: "Alice_1", dotall: "a\nb", no_dotall: "aXb",
  code: "ABC", handle: "Some.User", loose: "???",
}

def messages(attrs)
  record = Article.new(attrs)
  [record.valid?, record.errors]
end

valid, msgs = messages(base)
raise "valid base rejected: #{msgs.inspect}" unless valid

# `i` (case-insensitive): Rails' FormatValidator runs the regex as
# written against `value.to_s`, error "is invalid".
valid, _ = messages(base.merge(name_i: "Alice_1"))
raise "Alice_1 should be valid" unless valid
_, name_i_bad = messages(base.merge(name_i: "al ice"))
raise "al ice: #{name_i_bad.inspect}" unless name_i_bad.any? { |m| m.include?("is invalid") }
valid, _ = messages(base.merge(name_i: "alice"))
raise "alice should be valid" unless valid

# `m` (Ruby's dot-matches-newline): `/\Aa.b\z/m` accepts "a\nb";
# the unflagged control at the same value does not.
valid, _ = messages(base.merge(dotall: "a\nb"))
raise "dotall a\\nb should be valid" unless valid
_, no_dotall_bad = messages(base.merge(no_dotall: "a\nb"))
raise "no_dotall a\\nb: #{no_dotall_bad.inspect}" unless no_dotall_bad.any? { |m| m.include?("is invalid") }

# A model constant assigned a flagged regex literal resolves to that
# literal (NAME_RE = /\A[a-z]+\z/i).
valid, _ = messages(base.merge(code: "ABC"))
raise "ABC should be valid" unless valid
_, code_bad = messages(base.merge(code: "ab1"))
raise "ab1: #{code_bad.inspect}" unless code_bad.any? { |m| m.include?("is invalid") }

# A constant interpolating another constant whose flags match flattens
# to the equivalent plain pattern (ONLY = /\A#{INNER}\z/i).
valid, _ = messages(base.merge(handle: "Some.User"))
raise "Some.User should be valid" unless valid
valid, _ = messages(base.merge(handle: "a..b"))
raise "a..b should be valid" unless valid
_, handle_bad1 = messages(base.merge(handle: "-bad"))
raise "-bad: #{handle_bad1.inspect}" unless handle_bad1.any? { |m| m.include?("is invalid") }
_, handle_bad2 = messages(base.merge(handle: "a b"))
raise "'a b': #{handle_bad2.inspect}" unless handle_bad2.any? { |m| m.include?("is invalid") }

# `x` is not a flag this build can reproduce on every target: the
# whole `validates :loose, …` call is a survey gap, not a partial
# check, so any value (including one the regex would reject) saves.
valid, _ = messages(base.merge(loose: "???"))
raise "loose (gap) should not be enforced" unless valid

puts "format flags passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("format flags passed"), "{}", run.stdout);
}

#[test]
fn format_with_unsupported_flag_is_a_reported_gap_not_a_silent_drop() {
    let warnings = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write("app/models/article.rb", MODEL)
        .ingest_warnings();
    assert!(
        warnings.iter().any(|message| message.contains("format: with")),
        "missing format: with gap warning for the `x`-flagged `loose` validation in {warnings:?}"
    );
}

/// The `i`/`m`-flagged `Literal::Regex` nodes `inline_format_check`
/// now builds (and, for `Roda`, the ingest-level `ValidationRule`
/// itself) reach every backend's own literal emitter — go/csharp/
/// kotlin/swift/python/crystal/elixir, not just the three with a
/// runtime harness here. None of them should choke on a flag that
/// used to never reach this far (every format validation carried
/// empty flags before this change).
#[test]
fn flagged_format_validations_emit_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [
        Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda,
    ] {
        let (_emitted, errors) = emit_and_run::real_blog()
            .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
            .write("app/models/article.rb", MODEL_WITHOUT_INTERPOLATED_CONSTANT)
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
