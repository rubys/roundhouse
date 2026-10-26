//! `errors.add` with an attribute or options known only at run time
//! (activerecord 8.1.4, oracle `errors_add.rb`): a validator's
//! `record.errors.add(attribute, :invalid)` gives "Body is invalid" (the
//! model's translated name when the locale has one: "Heading is invalid"),
//! `count:` and `value:` interpolate into the locale's message (and
//! `count:` picks its plural form), and a String message is used as
//! given; on `:base` the message stands alone.
//!
//! These sites were reported as `errors.add` "left as dynamic dispatch",
//! and the emitted call raised NoMethodError (`add` on the Array the
//! errors are) when it ran.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const LOCALE: &str = "en:\n  hello: \"Hello world\"\n  activerecord:\n    attributes:\n      article:\n        title: \"Heading\"\n    errors:\n      messages:\n        too_many_words: \"has more than %{count} words\"\n        taken_value: \"%{value} is taken\"\n        one_or_more:\n          one: \"needs one item\"\n          other: \"needs %{count} items\"\n";

const CHECKER: &str = "class Checker\n  def self.invalid(record, attribute)\n    record.errors.add(attribute, :invalid)\n  end\n\n  def self.blank(record, attribute)\n    record.errors.add(attribute, :blank)\n  end\n\n  def self.words(record, attribute, limit)\n    record.errors.add(attribute, :too_many_words, count: limit)\n  end\n\n  def self.taken(record, attribute, value)\n    record.errors.add(attribute, :taken_value, value: value)\n  end\n\n  def self.items(record, attribute, n)\n    record.errors.add(attribute, :one_or_more, count: n)\n  end\n\n  def self.wrong(record, attribute)\n    record.errors.add(attribute, \"looks wrong\")\n  end\nend\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def fresh = Article.new(title: "t", body: "A body long enough to validate.")
a = fresh; Checker.invalid(a, :body)
check("a variable attribute, :invalid", [a.errors, a.error_details.map { |d| [d.attribute, d.kind] }], [["Body is invalid"], [["body", "invalid"]]])
a = fresh; Checker.invalid(a, :title)
check("the model's translated attribute name", a.errors, ["Heading is invalid"])
a = fresh; Checker.blank(a, :body)
check(":blank", a.errors, ["Body can't be blank"])
a = fresh; Checker.words(a, :body, 3)
check("count: interpolated", a.errors, ["Body has more than 3 words"])
a = fresh; Checker.taken(a, :body, "abc")
check("value: interpolated", a.errors, ["Body abc is taken"])
a = fresh; Checker.items(a, :body, 1)
check("count: 1 picks one", a.errors, ["Body needs one item"])
a = fresh; Checker.items(a, :body, 2)
check("count: 2 picks other", a.errors, ["Body needs 2 items"])
a = fresh; Checker.wrong(a, :body)
check("a String message", a.errors, ["Body looks wrong"])
a = fresh; Checker.invalid(a, :base)
check(":base stands alone", a.errors, ["is invalid"])
puts "dynamic errors.add passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", LOCALE)
        .write("app/models/checker.rb", CHECKER)
}

#[test]
fn errors_add_with_a_runtime_attribute_or_options_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("dynamic errors.add passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_add_with_a_runtime_attribute_or_options_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("dynamic errors.add passed"), "{}", run.stdout);
}

// `errors.add(:base, <method call>)`: a literal `:base` with a computed
// message is grounded already (the message is evaluated once into a
// local); kept here as the shape a large app's model uses.
const BASE_SCRIPT: &str = r##"
a = Article.new(title: "admin", body: "A body long enough to validate.")
got = [a.valid?, a.errors, a.error_details.map { |d| [d.attribute, d.kind] }]
want = [false, ["the title admin is reserved"], [["base", "the title admin is reserved"]]]
raise "base: #{got.inspect}" unless got == want
puts "base message passed"
"##;

#[test]
fn errors_add_on_base_with_a_method_call_message_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  validate :not_reserved\n\n  def not_reserved\n    errors.add(:base, reserved_message) if title == \"admin\"\n  end\n\n  def reserved_message\n    \"the title \" + title.to_s + \" is reserved\"\n  end\n",
        )
        .run_ruby(BASE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("base message passed"), "{}", run.stdout);
}

// Rails' lookup order for a run-time `errors.add(attribute, kind)`
// (oracle `lookup_order.rb`): the model's `attributes.<attr>.<kind>`, its
// `<kind>`, `activerecord.errors.messages`, `errors.attributes.<attr>`,
// then `errors.messages`. An abstract parent's key isn't consulted, and a
// tableless form's scope is `activemodel`.
const LOOKUP_LOCALE: &str = "en:\n  hello: \"Hello world\"\n  errors:\n    attributes:\n      title:\n        unusable: \"is unusable (errors.attributes)\"\n        worn: \"is worn (errors.attributes)\"\n    messages:\n      stale: \"is stale (errors.messages)\"\n      worn: \"is worn (errors.messages)\"\n  activerecord:\n    errors:\n      messages:\n        stale: \"is stale (activerecord.errors.messages)\"\n        unusable: \"is unusable (activerecord.errors.messages)\"\n      models:\n        article:\n          reserved: \"is reserved for articles\"\n          attributes:\n            slug:\n              invalid: \"is not a valid slug\"\n        titled_record:\n          reserved: \"is reserved for titled records\"\n        note:\n          reserved: \"is reserved for notes\"\n  activemodel:\n    errors:\n      messages:\n        stale: \"is stale (activemodel.errors.messages)\"\n";

// A literal kind in each branch, as a validator writes it.
const LOOKUP_CHECKER: &str = "class LookupChecker\n  def self.add(record, attribute, kind)\n    case kind\n    when :invalid then record.errors.add(attribute, :invalid)\n    when :reserved then record.errors.add(attribute, :reserved)\n    when :unusable then record.errors.add(attribute, :unusable)\n    when :stale then record.errors.add(attribute, :stale)\n    when :worn then record.errors.add(attribute, :worn)\n    when :blank then record.errors.add(attribute, :blank)\n    end\n  end\nend\n";

const LOOKUP_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def msg(record, attribute, kind) = record.tap { |r| LookupChecker.add(r, attribute, kind) }.errors
check("1. the model's attribute override", msg(Article.new, :slug, :invalid), ["Slug is not a valid slug"])
check("2. the model's kind override", msg(Article.new, :body, :reserved), ["Body is reserved for articles"])
check("3. activerecord.errors.messages, over errors.attributes", msg(Article.new, :title, :unusable), ["Title is unusable (activerecord.errors.messages)"])
check("3. activerecord.errors.messages, over errors.messages", msg(Article.new, :body, :stale), ["Body is stale (activerecord.errors.messages)"])
check("4. errors.attributes, over errors.messages", msg(Article.new, :title, :worn), ["Title is worn (errors.attributes)"])
check("5. errors.messages", msg(Article.new, :body, :worn), ["Body is worn (errors.messages)"])
check("5. Rails' default", msg(Article.new, :body, :blank), ["Body can't be blank"])
check("another model, the same kind: its own", msg(Note.new, :body, :reserved), ["Body is reserved for notes"])
check("an abstract parent's key isn't consulted", msg(Note.new, :body, :invalid), ["Body is invalid"])
check("a tableless form reads activemodel.errors.messages", msg(Form::Draft.new, :name, :stale), ["Name is stale (activemodel.errors.messages)"])
puts "lookup order passed"
"##;

fn lookup_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", LOOKUP_LOCALE)
        .write("app/models/lookup_checker.rb", LOOKUP_CHECKER)
        .write("app/models/titled_record.rb", "class TitledRecord < ApplicationRecord\n  self.abstract_class = true\nend\n")
        .write("app/models/note.rb", "class Note < TitledRecord\n  self.table_name = \"comments\"\nend\n")
        .write("app/models/form/draft.rb", "class Form::Draft\n  include ActiveModel::Model\n\n  attr_accessor :name\n\n  validates :name, presence: true\nend\n")
}

#[test]
fn a_runtime_errors_add_follows_rails_lookup_order_on_ruby() {
    let run = lookup_app().run_ruby(LOOKUP_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("lookup order passed"), "{}", run.stdout);
}

// Review of #1377 (oracle `cr_oracle.rb`): a value is substituted as
// written (no backslash expansion), a plural-only `errors.attributes`
// key is found, and `%{attribute}` / `%{model}` default to the human
// names, an explicit `attribute:` option winning.
const CR_LOCALE: &str = "en:\n  hello: \"Hello world\"\n  errors:\n    attributes:\n      title:\n        many_items:\n          one: \"needs one (attributes)\"\n          other: \"needs %{count} (attributes)\"\n    messages:\n      many_items:\n        one: \"needs one (messages)\"\n        other: \"needs %{count} (messages)\"\n  activerecord:\n    models:\n      article:\n        one: \"Story\"\n        other: \"Stories\"\n    errors:\n      messages:\n        taken_value: \"%{value} is taken\"\n        named: \"of %{attribute} is wrong\"\n        modeled: \"is wrong for a %{model}\"\n        both: \"%{value} / %{count}\"\n        percent: \"100%% of %{count}\"\n";

const CR_CHECKER: &str = "class CrChecker\n  def self.taken(record, attribute, value)\n    record.errors.add(attribute, :taken_value, value: value)\n  end\n\n  def self.items(record, attribute, n)\n    record.errors.add(attribute, :many_items, count: n)\n  end\n\n  def self.named(record, attribute)\n    record.errors.add(attribute, :named)\n  end\n\n  def self.renamed(record, attribute)\n    record.errors.add(attribute, :named, attribute: \"Text\")\n  end\n\n  def self.modeled(record, attribute)\n    record.errors.add(attribute, :modeled)\n  end\n\n  def self.both(record, attribute, value, n)\n    record.errors.add(attribute, :both, value: value, count: n)\n  end\n\n  def self.percent(record, attribute, n)\n    record.errors.add(attribute, :percent, count: n)\n  end\nend\n";

const CR_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def fresh = Article.new(title: "t", body: "A body long enough to validate.")
a = fresh; CrChecker.taken(a, :body, "a\\0b")
check("a backslash-zero value, verbatim", a.errors, ["Body a\\0b is taken"])
a = fresh; CrChecker.taken(a, :body, "x\\&y")
check("a backslash-ampersand value, verbatim", a.errors, ["Body x\\&y is taken"])
a = fresh; CrChecker.items(a, :title, 1)
check("a plural-only errors.attributes key, one", a.errors, ["Title needs one (attributes)"])
a = fresh; CrChecker.items(a, :title, 2)
check("a plural-only errors.attributes key, other", a.errors, ["Title needs 2 (attributes)"])
a = fresh; CrChecker.items(a, :body, 2)
check("another attribute: errors.messages", a.errors, ["Body needs 2 (messages)"])
a = fresh; CrChecker.named(a, :body)
check("%{attribute} by default", a.errors, ["Body of Body is wrong"])
a = fresh; CrChecker.renamed(a, :body)
check("%{attribute} given", a.errors, ["Body of Text is wrong"])
a = fresh; CrChecker.modeled(a, :body)
check("%{model} by default", a.errors, ["Body is wrong for a Story"])
a = fresh; CrChecker.both(a, :body, "%{count}", 3)
check("an inserted value is not rescanned", a.errors, ["Body %{count} / 3"])
a = fresh; CrChecker.percent(a, :body, 3)
check("%% is a literal %", a.errors, ["Body 100% of 3"])
puts "review rows passed"
"##;

#[test]
fn errors_add_review_rows_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("config/locales/en.yml", CR_LOCALE)
        .write("app/models/cr_checker.rb", CR_CHECKER)
        .run_ruby(CR_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("review rows passed"), "{}", run.stdout);
}

// Review of #1377 (oracle `cr_oracle_verbatim.rb`): a String message is
// kept as written, `%{…}` and all; only a Symbol kind's template is
// interpolated.
const VERBATIM_CHECKER: &str = "class VerbatimChecker\n  def self.literal(record)\n    record.errors.add(:body, \"literal %{attribute}\")\n  end\n\n  def self.variable(record, attribute)\n    record.errors.add(attribute, \"literal %{attribute} %{model}\")\n  end\n\n  def self.base(record, attribute)\n    record.errors.add(attribute, \"base %{attribute}\")\n  end\n\n  def self.symbol(record, attribute)\n    record.errors.add(attribute, :named)\n  end\nend\n";

const VERBATIM_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def fresh = Article.new(title: "t", body: "A body long enough to validate.")
a = fresh; VerbatimChecker.literal(a)
check("a literal attribute, a String with %{attribute}", a.errors, ["Body literal %{attribute}"])
a = fresh; VerbatimChecker.variable(a, :body)
check("a variable attribute, a String with %{attribute}", a.errors, ["Body literal %{attribute} %{model}"])
a = fresh; VerbatimChecker.base(a, :base)
check(":base, a String with %{attribute}", a.errors, ["base %{attribute}"])
a = fresh; VerbatimChecker.symbol(a, :body)
check("a Symbol kind still interpolates", a.errors, ["Body of Body is wrong"])
puts "verbatim rows passed"
"##;

#[test]
fn a_string_message_is_kept_verbatim_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("config/locales/en.yml", CR_LOCALE)
        .write("app/models/verbatim_checker.rb", VERBATIM_CHECKER)
        .run_ruby(VERBATIM_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("verbatim rows passed"), "{}", run.stdout);
}

// A computed String message with a run-time attribute (`validate_each`'s
// `errors.add(attribute, "saw " + value.inspect)`): evaluated once, and
// kept as written (`cr_oracle_verbatim.rb`, "a computed String").
const COMPUTED_CHECKER: &str = "class ComputedChecker\n  def self.saw(record, attribute, value)\n    record.errors.add(attribute, \"saw \" + value.to_s)\n  end\nend\n";

const COMPUTED_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = Article.new(title: "t", body: "A body long enough to validate.")
ComputedChecker.saw(a, :body, "%{count}")
check("a computed String, verbatim", a.errors, ["Body saw %{count}"])
puts "computed rows passed"
"##;

#[test]
fn a_computed_string_message_with_a_runtime_attribute_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("app/models/computed_checker.rb", COMPUTED_CHECKER)
        .run_ruby(COMPUTED_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("computed rows passed"), "{}", run.stdout);
}
