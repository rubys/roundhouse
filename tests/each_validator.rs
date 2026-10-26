//! `validates :slug, presence: true, slug: { reserved: %w[admin new] }`,
//! a key naming an app `ActiveModel::EachValidator`. Rails 8.1.4 (oracle
//! `each_validator.rb`, lookup `resolve.rb`): the key's validator gets
//! `attributes:` and the key's options (`true` is none, a Hash is them,
//! another value is `with:`), and `validate_each(record, attribute,
//! value)` runs per attribute after the call's built-in checks, skipped
//! for a nil value under `allow_nil:` and a blank one under
//! `allow_blank:`, gated by `if:` / `unless:` / `on:`. `foo:` names
//! `<Model>::FooValidator`, then a superclass's, then `FooValidator`;
//! `"admin/foo":` names `Admin::FooValidator`.
//!
//! The key was reported and dropped with the call's other rules, and the
//! validator class was dropped as a subclass of an unported Rails base.
//! A key naming no app validator, and a strict target, keep the report.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA_ANCHOR: &str = "  create_table \"comments\", force: :cascade do |t|\n";
const SCHEMA: &str = "  create_table \"entries\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"slug\"\n    t.string \"code\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\", force: :cascade do |t|\n";

const LOCALE: &str = "en:\n  activerecord:\n    errors:\n      messages:\n        reserved: \"is reserved\"\n        too_many_words: \"has more than %{count} words\"\n";

const SLUG: &str = "class SlugValidator < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    return if value.to_s.empty?\n    record.errors.add(attribute, :invalid) unless value.to_s.match?(/\\A[a-z0-9-]+\\z/)\n    reserved = options[:reserved] || []\n    record.errors.add(attribute, :reserved) if reserved.include?(value.to_s)\n  end\nend\n";
const WORDS: &str = "class WordsValidator < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    maximum = options[:maximum]\n    record.errors.add(attribute, :too_many_words, count: maximum) if value.to_s.split.size > maximum\n  end\nend\n";
const NIL_SEEN: &str = "class NilSeenValidator < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    record.errors.add(attribute, \"saw \" + value.inspect)\n  end\nend\n";
const PREFIX: &str = "class PrefixValidator < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    prefix = options[:with].to_s\n    record.errors.add(attribute, \"must start with \" + prefix) unless value.to_s.start_with?(prefix)\n  end\nend\n";

const ENTRY: &str = "class Entry < ApplicationRecord\n  validates :slug, presence: true, slug: { reserved: %w[admin new] }\n  validates :body, words: { maximum: 3 }, allow_nil: true\n  validates :title, nil_seen: true, if: -> { slug == \"probe-nil\" }\n  validates :code, prefix: \"X-\", on: :create\nend\n";
const NOTE: &str = "class Note < ApplicationRecord\n  self.table_name = \"entries\"\n  validates :title, :code, slug: true\nend\n";
const MEMO: &str = "class Memo < ApplicationRecord\n  self.table_name = \"entries\"\n  validates :title, nil_seen: true, allow_blank: true\nend\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def messages(record)
  record.valid?
  record.errors
end
check("invalid value: message", messages(Entry.new(slug: "Bad Slug", code: "X-1")), ["Slug is invalid"])
check("valid value", Entry.new(slug: "good-slug", code: "X-1").valid?, true)
check("an option hash read in validate_each", messages(Entry.new(slug: "admin", code: "X-1")), ["Slug is reserved"])
check("a custom message key from the locale, with count:", messages(Entry.new(slug: "s", code: "X-1", body: "one two three four")), ["Body has more than 3 words"])
check("allow_nil: skips nil", Entry.new(slug: "s", code: "X-1", body: nil).valid?, true)
check("without allow_nil, nil reaches validate_each", messages(Entry.new(slug: "probe-nil", code: "X-1")), ["Title saw nil"])
check("allow_blank: skips \"\"", Memo.new(title: "").valid?, true)
check("allow_blank: runs on a value", messages(Memo.new(title: "x")), ["Title saw \"x\""])
check("presence then the custom check, in order", messages(Entry.new(slug: "", code: "X-1")), ["Slug can't be blank"])
check("a non-hash value is options[:with]", messages(Entry.new(slug: "s", code: "Y-1")), ["Code must start with X-"])
saved = Entry.create!(slug: "s", code: "X-1")
saved.code = "Y-1"
check("on: :create, skipped on update", saved.valid?, true)
check("several attributes, one validator", messages(Note.new(title: "Bad T", code: "Bad C")), ["Title is invalid", "Code is invalid"])
built = SlugValidator.new(attributes: [:slug], reserved: ["x"])
check("the options a validator gets", [built.attributes, built.options], [[:slug], { reserved: ["x"] }])
puts "each validator passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_ANCHOR, SCHEMA)
        .write("config/locales/validators.en.yml", LOCALE)
        .write("app/models/entry.rb", ENTRY)
        .write("app/models/note.rb", NOTE)
        .write("app/models/memo.rb", MEMO)
        .write("app/validators/slug_validator.rb", SLUG)
        .write("app/validators/words_validator.rb", WORDS)
        .write("app/validators/nil_seen_validator.rb", NIL_SEEN)
        .write("app/validators/prefix_validator.rb", PREFIX)
}

#[test]
fn a_custom_key_runs_its_each_validator_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("each validator passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_custom_key_runs_its_each_validator_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("each validator passed"), "{}", run.stdout);
}

// Rails' lookup (`resolve.rb`): the model's own constant wins over a
// top-level one, a superclass's constant is found, and a slash key names
// a namespaced class.
const LOOKUP_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def messages(record)
  record.valid?
  record.errors
end
check("the model's own constant wins over top-level", messages(Shop::Item.new), ["Title own mark"])
check("a superclass's constant", messages(Leaf.new), ["Title parent mark"])
check("a slash key", messages(Slashed.new), ["Title admin mark"])
puts "lookup passed"
"##;

fn marker(class: &str, text: &str) -> String {
    format!("class {class} < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    record.errors.add(attribute, \"{text}\")\n  end\nend\n")
}

#[test]
fn a_key_resolves_as_rails_looks_the_constant_up_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_ANCHOR, SCHEMA)
        .write("app/validators/mark_validator.rb", &marker("MarkValidator", "top mark"))
        .write("app/models/shop/item/mark_validator.rb", &marker("Shop::Item::MarkValidator", "own mark"))
        .write("app/models/shop/item.rb", "module Shop\n  class Item < ApplicationRecord\n    self.table_name = \"entries\"\n    validates :title, mark: true\n  end\nend\n")
        .write("app/models/branch.rb", "class Branch < ApplicationRecord\n  self.abstract_class = true\nend\n")
        .write("app/models/branch/twig_validator.rb", &marker("Branch::TwigValidator", "parent mark"))
        .write("app/models/leaf.rb", "class Leaf < Branch\n  self.table_name = \"entries\"\n  validates :title, twig: true\nend\n")
        .write("app/validators/admin/mark_validator.rb", &marker("Admin::MarkValidator", "admin mark"))
        .write("app/models/slashed.rb", "class Slashed < ApplicationRecord\n  self.table_name = \"entries\"\n  validates :title, \"admin/mark\": true\nend\n")
        .run_ruby(LOOKUP_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("lookup passed"), "{}", run.stdout);
}

// A key naming no app class (a gem's validator) is reported at ingest, as
// before, and the whole call dropped, its `presence:` with it.
#[test]
fn a_key_naming_no_app_validator_stays_reported() {
    let warnings = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_ANCHOR, SCHEMA)
        .write("app/models/entry.rb", "class Entry < ApplicationRecord\n  validates :slug, presence: true, nowhere: true\nend\n")
        .ingest_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("validates :slug, nowhere:") && w.contains("validation option not lowered")),
        "{warnings:#?}"
    );
}

/// The `Entry` body after `target`'s lowering, and the warnings it gave.
fn lowered_entry(target: BuildTarget) -> (Vec<String>, Vec<String>) {
    let root = std::env::temp_dir().join(format!("roundhouse-each-validator-{}-{}", std::process::id(), target.as_str()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let schema = std::fs::read_to_string(source.join("db/schema.rb")).unwrap().replacen(SCHEMA_ANCHOR, SCHEMA, 1);
    std::fs::write(source.join("db/schema.rb"), schema).unwrap();
    std::fs::write(source.join("app/models/entry.rb"), ENTRY).unwrap();
    for (name, text) in [("slug", SLUG), ("words", WORDS), ("nil_seen", NIL_SEEN), ("prefix", PREFIX)] {
        std::fs::create_dir_all(source.join("app/validators")).unwrap();
        std::fs::write(source.join(format!("app/validators/{name}_validator.rb")), text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, target);
    std::fs::remove_dir_all(&root).ok();
    let entry = app.models.iter().find(|m| m.name.0.as_str() == "Entry").expect("Entry");
    let items = entry.body.iter().map(|item| format!("{item:?}")).collect();
    let warnings = ingest.into_iter().chain(lowered).map(|d| d.message).filter(|m| m.contains("validates :")).collect();
    (items, warnings)
}

// A strict target keeps the report, for each attribute and key, and
// drops each call with its built-in rules, as an ingest gap does.
#[test]
fn a_strict_target_reports_the_keys_and_drops_the_calls() {
    let (items, warnings) = lowered_entry(BuildTarget::Typescript);
    assert!(items.is_empty(), "{items:#?}");
    for construct in ["validates :slug, slug:", "validates :body, words:", "validates :title, nil_seen:", "validates :code, prefix:"] {
        assert!(
            warnings.iter().any(|w| w.contains(construct) && w.contains("validation option not lowered")),
            "{construct}: {warnings:#?}"
        );
    }
}

// The ruby family reports none of them: ingest keeps the keys quiet.
#[test]
fn the_ruby_family_reports_no_key() {
    let (_items, warnings) = lowered_entry(BuildTarget::Ruby);
    assert!(warnings.is_empty(), "{warnings:#?}");
}

// Review of #1381 (oracle `cr_oracle.rb`). Rails' `_parse_validates_options`
// passes a Range as `in:`, as an Array.
const SIZE: &str = "class SizeValidator < ActiveModel::EachValidator\n  def validate_each(record, attribute, value)\n    record.errors.add(attribute, \"options \" + options.inspect)\n  end\nend\n";

#[test]
fn a_range_value_is_in_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_ANCHOR, SCHEMA)
        .write("app/validators/size_validator.rb", SIZE)
        .write("app/models/entry.rb", "class Entry < ApplicationRecord\n  validates :title, size: 1..5\nend\n")
        .run_ruby("e = Entry.new\ne.valid?\nraise e.errors.inspect unless e.errors == [\"Title options {in: 1..5}\"]\nputs \"range passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("range passed"), "{}", run.stdout);
}

// A constant or a call is classified by its class at run time (a Hash is
// the options, an Array `in:`), which the build doesn't know: the call
// stays reported.
#[test]
fn a_key_whose_value_is_a_constant_stays_reported() {
    let warnings = lowered_warnings(
        &[
            ("app/validators/size_validator.rb", SIZE),
            ("app/models/entry.rb", "class Entry < ApplicationRecord\n  OPTIONS = { max: 3 }\n  validates :title, size: OPTIONS\nend\n"),
        ],
        BuildTarget::Ruby,
    );
    assert!(
        warnings.iter().any(|w| w.contains("validates :title, size:") && w.contains("validation option not lowered")),
        "{warnings:#?}"
    );
}

// The class reference is root-qualified: in a model inside a module, a
// bare name would find the module's class of that name, which Rails'
// lookup doesn't consult (oracle: "top mark").
#[test]
fn a_model_in_a_module_gets_the_top_level_validator_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_ANCHOR, SCHEMA)
        .write("app/validators/mark_validator.rb", &marker("MarkValidator", "top mark"))
        .write("app/models/shop/mark_validator.rb", &marker("Shop::MarkValidator", "module mark"))
        .write("app/models/shop/item.rb", "module Shop\n  class Item < ApplicationRecord\n    self.table_name = \"entries\"\n    validates :title, mark: true\n  end\nend\n")
        .run_ruby("i = Shop::Item.new\ni.valid?\nraise i.errors.inspect unless i.errors == [\"Title top mark\"]\nputs \"module passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("module passed"), "{}", run.stdout);
}

/// The `validates` warnings ingest and `target`'s lowering give for the
/// blog with `files` written over it.
fn lowered_warnings(files: &[(&str, &str)], target: BuildTarget) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("roundhouse-each-validator-w-{}-{}", std::process::id(), files.len()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let schema = std::fs::read_to_string(source.join("db/schema.rb")).unwrap().replacen(SCHEMA_ANCHOR, SCHEMA, 1);
    std::fs::write(source.join("db/schema.rb"), schema).unwrap();
    for (path, text) in files {
        let full = source.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, target);
    std::fs::remove_dir_all(&root).ok();
    ingest.into_iter().chain(lowered).map(|d| d.message).filter(|m| m.contains("validates :")).collect()
}
