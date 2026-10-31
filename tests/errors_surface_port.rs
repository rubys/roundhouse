//! PostgreSQL: errors surface port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;
use std::process::Command;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.0].define(version: 1) do
  create_table "posts", force: :cascade do |t|
    t.string "title"
    t.integer "author_id"
  end
  create_table "entries", force: :cascade do |t|
    t.string "title"
    t.string "code"
    t.string "kind"
    t.string "pages"
    t.integer "rank"
    t.integer "post_id"
  end
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE posts (
    id bigserial NOT NULL,
    title character varying,
    author_id bigint,
    CONSTRAINT posts_pkey PRIMARY KEY (id)
);

CREATE TABLE entries (
    id bigserial NOT NULL,
    title character varying,
    code character varying,
    kind character varying,
    pages character varying,
    rank integer,
    post_id bigint,
    CONSTRAINT entries_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const POST: &str = r#"class Post < ApplicationRecord
  validates :title, presence: true
  validates :title, length: { maximum: 12 }
  validate :checks

  def checks
    errors.add(:base, "no shouting") if title == "LOUD"
    errors.add(:author_id, "must be set") if title == "LOUD"
  end
end
"#;

/// A validator of each kind whose options the push records, and each
/// `errors.add` shape the lowering grounds. `pages` is text: on an
/// integer column Rails' numericality reads the value before type cast,
/// which the PostgreSQL tree's cast-on-write attribute does not keep.
const ENTRY: &str = r#"class Entry < ApplicationRecord
  belongs_to :post, optional: false
  validates :title, presence: true, length: { minimum: 2, maximum: 8 }
  validates :code, format: { with: /\A[A-Z]+\z/ }, allow_nil: true
  validates :kind, inclusion: { in: %w[memo note spam] }
  validates :kind, exclusion: { in: %w[spam] }
  validates :pages, numericality: true, allow_nil: true
  validates :terms, acceptance: true
  validates :rank, comparison: { greater_than: 2 }, allow_nil: true
  validates :code, uniqueness: true, allow_nil: true
  validate :checks

  def checks
    errors.add(:base, "custom text") if title == "base"
    errors.add(:code, :blank) if title == "typed"
    errors.add(:code) if title == "bare"
    errors.add(:code, message: "named") if title == "named"
  end
end
"#;

const POST_CHECK: &str = r#"class PostCheck
  include ActiveModel::Validations

  attr_accessor :name

  validates :name, presence: true

  def initialize(name)
    @name = name
  end
end
"#;

/// App code, so the calls go through the compiler. Each line is one
/// answer, or the error it raised.
const PROBE: &str = r##"class SurfaceProbe
  def self.show(out, label)
    value = begin
      yield
    rescue StandardError => e
      [e.class.name, e.message]
    end
    out << "#{label}: #{value.inspect}"
  end

  def self.invalid(title)
    post = Post.new(title: title)
    post.valid?
    post
  end

  def self.check(name)
    check = PostCheck.new(name)
    check.valid?
    check
  end

  def self.lines
    out = []
    show(out, "merge records") do
      post = invalid("")
      post.errors.merge!(invalid("LOUD").errors)
      [post.errors.full_messages, post.errors.to_hash, post.errors.attribute_names]
    end
    show(out, "merge answers") do
      post = invalid("")
      post.errors.merge!(invalid("LOUD").errors).map(&:full_message)
    end
    show(out, "merge itself") do
      post = invalid("LOUD")
      post.errors.merge!(post.errors)
      post.errors.to_hash(true)
    end
    show(out, "merge a valid record") do
      post = invalid("")
      post.errors.merge!(invalid("fine").errors)
      post.errors.full_messages
    end
    show(out, "record takes a validator's") do
      post = invalid("LOUD")
      post.errors.merge!(check("").errors)
      [post.errors.full_messages, post.errors.to_hash]
    end
    show(out, "validator takes a record's") do
      found = check(nil)
      found.errors.merge!(invalid("LOUD").errors)
      [found.errors.full_messages, found.errors.to_hash, found.errors.attribute_names]
    end
    show(out, "details of each validator") do
      entry = Entry.new(title: "x", code: "abc", kind: "spam", pages: "lots", terms: "0", rank: 1)
      entry.valid?
      [entry.errors.details, entry.errors.details[:missing]]
    end
    show(out, "details of a long title") do
      entry = Entry.new(title: "far too long", kind: "memo", post: Post.new(title: "p"))
      entry.valid?
      entry.errors.details
    end
    show(out, "details of each add") do
      %w[base typed bare named].map do |title|
        entry = Entry.new(title: title, kind: "memo", post: Post.new(title: "p"))
        entry.valid?
        entry.errors.details
      end
    end
    show(out, "details after merge") do
      post = invalid("")
      post.errors.merge!(invalid("LOUD").errors)
      post.errors.details
    end
    show(out, "each over the errors") do
      entry = Entry.new(title: "x", code: "abc", kind: "spam", pages: "lots", terms: "0", rank: 1)
      entry.valid?
      seen = []
      entry.errors.each { |error| seen << [error.attribute, error.type, error.message] }
      seen
    end
    show(out, "map over the errors") do
      post = invalid("LOUD")
      [post.errors.map(&:full_message), post.errors.map { |error| [error.attribute, error.raw_type] },
       post.errors.collect(&:details), post.errors.flat_map { |error| [error.attribute, error.message] }]
    end
    show(out, "queries over the errors") do
      post = invalid("LOUD")
      [post.errors.first.attribute, post.errors.find { |error| error.attribute == :author_id }.full_message,
       post.errors.any? { |error| error.attribute == :base }, post.errors.none? { |error| error.type == :blank },
       post.errors.count { |error| error.attribute == :base }, post.errors.one? { |error| error.attribute == :base },
       post.errors.all? { |error| error.match?(error.attribute) }, post.errors.find_index { |error| error.attribute == :author_id },
       post.errors.one?, invalid("").errors.one?, post.errors.all?]
    end
    show(out, "collections of the errors") do
      post = invalid("LOUD")
      [post.errors.select { |error| error.attribute == :base }.map(&:message),
       post.errors.reject { |error| error.attribute == :base }.map(&:message),
       post.errors.filter_map { |error| error.message if error.attribute == :author_id },
       post.errors.group_by(&:attribute).transform_values { |errors| errors.map(&:message) },
       post.errors.each_with_object({}) { |error, out| out[error.attribute] = error.type },
       post.errors.sort_by { |error| error.attribute.to_s }.map(&:attribute),
       post.errors.to_a.size, post.errors.errors.map(&:attribute), post.errors.objects.map(&:message)]
    end
    show(out, "queries by attribute") do
      entry = Entry.new(title: "x", code: "abc", kind: "spam", pages: "lots", terms: "0", rank: 1)
      entry.valid?
      errors = entry.errors
      [errors.include?(:title), errors.include?(:missing), errors.key?("kind"), errors.has_key?(:rank), errors.exclude?(:title),
       errors.of_kind?(:title, :too_short), errors.of_kind?(:title), errors.of_kind?(:title, "is too short (minimum is 2 characters)"),
       errors.added?(:title, :too_short, count: 2), errors.added?(:title, :too_short), errors.added?(:terms, :accepted),
       errors.where(:title).map(&:type), errors.where(:rank, :greater_than, count: 2).size, errors.where(:rank, count: 3).size,
       errors.messages_for(:kind), errors.full_messages_for(:code), errors.full_messages_for(:missing).frozen?,
       errors.full_message(:post_id, "is odd"), errors.full_message(:base, "is odd")]
    end
    show(out, "queries on text messages") do
      post = invalid("LOUD")
      [post.errors.of_kind?(:base, "no shouting"), post.errors.added?(:author_id, "must be set"),
       post.errors.added?(:base, :invalid), post.errors.where(:base, "no shouting").map(&:full_message)]
    end
    show(out, "delete") do
      entry = Entry.new(title: "x", code: "abc", kind: "spam", pages: "lots", terms: "0", rank: 1)
      entry.valid?
      [entry.errors.delete(:title), entry.errors.delete(:kind, :inclusion), entry.errors.delete(:kind, :exclusion),
       entry.errors.delete(:missing), entry.errors.delete("rank", :greater_than, count: 2),
       entry.errors.full_messages, entry.errors.attribute_names]
    end
    show(out, "import") do
      post = invalid("")
      other = invalid("LOUD")
      post.errors.import(other.errors.first)
      post.errors.import(other.errors.where(:author_id).first, attribute: "title", type: "unset")
      [post.errors.full_messages, post.errors.details, post.errors.map { |error| [error.attribute, error.type, error.message] },
       post.errors.of_kind?(:title, :unset), post.errors.added?(:title, "must be set")]
    end
    show(out, "a validator's errors") do
      found = check(nil)
      [found.errors.map(&:full_message), found.errors.map(&:details), found.errors.first.strict_match?(:name, :blank)]
    end
    show(out, "then revalidated") do
      post = invalid("")
      post.errors.merge!(invalid("LOUD").errors)
      post.title = "fine"
      post.valid?
      post.errors.to_hash
    end
    out
  end
end
"##;

/// Rails' answers, spelled out so a run that drifts from native in the
/// same way on both sides still fails.
const EXPECTED: &[&str] = &[
    r#"merge records: [["Title can't be blank", "no shouting", "Author must be set"], {title: ["can't be blank"], base: ["no shouting"], author_id: ["must be set"]}, [:title, :base, :author_id]]"#,
    r#"merge answers: ["no shouting", "Author must be set"]"#,
    r#"merge itself: {base: ["no shouting"], author_id: ["Author must be set"]}"#,
    r#"merge a valid record: ["Title can't be blank"]"#,
    r#"record takes a validator's: [["no shouting", "Author must be set", "Name can't be blank"], {base: ["no shouting"], author_id: ["must be set"], name: ["can't be blank"]}]"#,
    r#"validator takes a record's: [["Name can't be blank", "no shouting", "Author must be set"], {name: ["can't be blank"], base: ["no shouting"], author_id: ["must be set"]}, [:name, :base, :author_id]]"#,
    r#"details of each validator: [{post: [{error: :blank}], title: [{error: :too_short, count: 2}], code: [{error: :invalid, value: "abc"}], kind: [{error: :exclusion, value: "spam"}], pages: [{error: :not_a_number, value: "lots"}], terms: [{error: :accepted}], rank: [{error: :greater_than, count: 2, value: 1}]}, []]"#,
    r#"details of a long title: {title: [{error: :too_long, count: 8}]}"#,
    r#"details of each add: [{base: [{error: "custom text"}]}, {code: [{error: :blank}]}, {code: [{error: :invalid}]}, {code: [{error: :invalid}]}]"#,
    r#"details after merge: {title: [{error: :blank}], base: [{error: "no shouting"}], author_id: [{error: "must be set"}]}"#,
    r#"each over the errors: [[:post, :blank, "must exist"], [:title, :too_short, "is too short (minimum is 2 characters)"], [:code, :invalid, "is invalid"], [:kind, :exclusion, "is reserved"], [:pages, :not_a_number, "is not a number"], [:terms, :accepted, "must be accepted"], [:rank, :greater_than, "must be greater than 2"]]"#,
    r#"map over the errors: [["no shouting", "Author must be set"], [[:base, "no shouting"], [:author_id, "must be set"]], [{error: "no shouting"}, {error: "must be set"}], [:base, "no shouting", :author_id, "must be set"]]"#,
    r#"queries over the errors: [:base, "Author must be set", true, true, 1, true, true, 1, false, true, true]"#,
    r#"collections of the errors: [["no shouting"], ["must be set"], ["must be set"], {base: ["no shouting"], author_id: ["must be set"]}, {base: "no shouting", author_id: "must be set"}, [:author_id, :base], 2, [:base, :author_id], ["no shouting", "must be set"]]"#,
    r#"queries by attribute: [true, false, true, true, false, true, false, true, true, false, true, [:too_short], 1, 0, ["is reserved"], ["Code is invalid"], true, "Post is odd", "is odd"]"#,
    r#"queries on text messages: [true, true, false, ["no shouting"]]"#,
    r#"delete: [["is too short (minimum is 2 characters)"], nil, ["is reserved"], nil, ["must be greater than 2"], ["Post must exist", "Code is invalid", "Pages is not a number", "Terms must be accepted"], [:post, :code, :pages, :terms]]"#,
    r#"import: [["Title can't be blank", "no shouting", "Title must be set"], {title: [{error: :blank}, {error: "must be set"}], base: [{error: "no shouting"}]}, [[:title, :blank, "can't be blank"], [:base, "no shouting", "no shouting"], [:title, :unset, "must be set"]], true, true]"#,
    r#"a validator's errors: [["Name can't be blank"], [{error: :blank}], true]"#,
    r#"then revalidated: {}"#,
];

const MODELS: &[(&str, &str)] =
    &[("application_record", APPLICATION_RECORD), ("post", POST), ("entry", ENTRY), ("post_check", POST_CHECK)];

fn expected() -> String {
    EXPECTED.iter().map(|l| format!("{l}\n")).collect()
}

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/surface_probe.rb", PROBE);
    for (name, source) in MODELS {
        app = app.write(&format!("app/models/{name}.rb"), source);
    }
    app
}

fn native() -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_errors_surface_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("schema.rb"), SCHEMA).unwrap();
    std::fs::write(dir.join("surface_probe.rb"), PROBE).unwrap();
    let mut requires = String::new();
    for (name, source) in MODELS {
        std::fs::write(dir.join(format!("{name}.rb")), source).unwrap();
        requires.push_str(&format!("require_relative {name:?}\n"));
    }
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", \"activemodel\", \"activesupport\")\n\
         require \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\n\
         load File.expand_path(\"schema.rb\", __dir__)\n\
         {requires}require_relative \"surface_probe\"\n\
         puts SurfaceProbe.lines\n"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .args(["-W0", "native.rb"])
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

#[test]
#[ignore = "fork spec: postgresql"]
fn errors_surface_runs_like_rails() {
    let run = app().run_ruby("puts SurfaceProbe.lines");
    run.assert_passes();
    let probe = std::fs::read_to_string(run.emitted.join("app/models/surface_probe.rb")).unwrap();
    assert!(probe.contains("ActiveModel::ErrorAttributes.merge!(post.errors, invalid(\"LOUD\").errors)"), "{probe}");
    assert_eq!(run.stdout, expected(), "emitted vs Rails' answers");
    assert_eq!(run.stdout, native(), "emitted vs native activerecord");
}

/// A uniqueness error's options hold the `existing_id` Rails found,
/// which the push does not record: `details` raises naming the message
/// rather than answer without it, while the messages stay readable.
const TAKEN_PROBE: &str = r##"class TakenProbe
  def self.lines
    post = Post.create!(title: "p")
    Entry.create!(title: "first", code: "AB", kind: "memo", post: post)
    entry = Entry.new(title: "second", code: "AB", kind: "memo", post: post)
    entry.valid?
    out = [entry.errors.full_messages.inspect]
    begin
      out << entry.errors.details.inspect
    rescue RuntimeError => e
      out << e.message
    end
    out
  end
end
"##;

#[test]
#[ignore = "fork spec: postgresql"]
fn unrecorded_options_raise_from_details() {
    let run = app().write("app/models/taken_probe.rb", TAKEN_PROBE).run_ruby("puts TakenProbe.lines");
    run.assert_passes();
    assert_eq!(
        run.stdout,
        "[\"Code has already been taken\"]\n\
         roundhouse: ActiveModel::Error: the options of \"Code has already been taken\" on :code was not recorded by the push that added it\n"
    );
}

/// Spinel emits no port and says so for each call the errors lowering does
/// not ground; the calls it grounds (`merge!` of a record, `details`, the
/// iteration of the record's `error_details`) it answers itself.
#[test]
#[ignore = "fork spec: postgresql"]
fn spinel_refuses_the_calls() {
    let (dir, errors) = app().emit(BuildTarget::Spinel);
    let refused = |call: &str| {
        errors.iter().any(|e| {
            e.contains(&format!("{call}`"))
                && e.contains("declared by the Ruby-family runtime (runtime/active_model_errors.rb), which spinel does not emit")
        })
    };
    for call in [
        "ActiveModel::Errors#group_by",
        "ActiveModel::Errors#where",
        "ActiveModel::Errors#delete",
        "ActiveModel::Errors#import",
    ] {
        assert!(refused(call), "spinel {call}: {errors:#?}");
    }
    for call in [
        "ActiveModel::Errors#merge!",
        "ActiveModel::Errors#details",
        "ActiveModel::Errors#each",
        "ActiveModel::Errors#map",
        "ActiveModel::Errors#first",
    ] {
        assert!(!refused(call), "spinel answers {call} from error_details: {errors:#?}");
    }
    assert!(!dir.join("runtime/active_model_errors.rb").exists());
}

/// On the PostgreSQL tree: a class whose superclass is no model, over
/// the ActiveModel::Validations port.
const CHECK_BASE: &str = "class CheckBase\nend\n";
const PORT_CHECK: &str = r#"class PortCheck < CheckBase
  include ActiveModel::Validations

  attr_accessor :name

  validates :name, presence: true

  def initialize(name)
    @name = name
  end
end
"#;

const PORT_PROBE: &str = r##"class PortProbe
  def self.show(out, label)
    value = begin
      yield
    rescue StandardError => e
      [e.class.name, e.message]
    end
    out << "#{label}: #{value.inspect}"
  end

  def self.lines
    out = []
    show(out, "record takes a port's") do
      post = Post.new(title: "LOUD")
      post.valid?
      check = PortCheck.new("")
      check.valid?
      post.errors.merge!(check.errors)
      [post.errors.full_messages, post.errors.to_hash]
    end
    show(out, "port takes a record's") do
      check = PortCheck.new(nil)
      check.valid?
      post = Post.new(title: "LOUD")
      post.valid?
      check.errors.merge!(post.errors)
      [check.errors.full_messages, check.errors.to_hash, check.errors.details]
    end
    show(out, "each side's errors") do
      check = PortCheck.new(nil)
      check.valid?
      post = Post.new(title: "LOUD")
      post.valid?
      check.errors.merge!(post.errors)
      [check.errors.map { |error| [error.attribute, error.type] }, post.errors.map(&:details),
       check.errors.select { |error| error.attribute == :base }.map(&:full_message), check.errors.first.message]
    end
    show(out, "each side's queries") do
      check = PortCheck.new(nil)
      check.valid?
      post = Post.new(title: "LOUD")
      post.valid?
      check.errors.import(post.errors.first, attribute: :name)
      post.errors.import(check.errors.first)
      [check.errors.include?(:name), check.errors.of_kind?(:name, :blank), check.errors.full_messages_for(:name),
       check.errors.delete(:name, :blank), check.errors.full_messages,
       post.errors.added?(:name, :blank), post.errors.where(:name).map(&:full_message), post.errors.delete(:base), post.errors.full_messages]
    end
    out
  end
end
"##;

const PORT_EXPECTED: &[&str] = &[
    r#"record takes a port's: [["no shouting", "Author must be set", "Name can't be blank"], {base: ["no shouting"], author_id: ["must be set"], name: ["can't be blank"]}]"#,
    r#"port takes a record's: [["Name can't be blank", "no shouting", "Author must be set"], {name: ["can't be blank"], base: ["no shouting"], author_id: ["must be set"]}, {name: [{error: :blank}], base: [{error: "no shouting"}], author_id: [{error: "must be set"}]}]"#,
    r#"each side's errors: [[[:name, :blank], [:base, "no shouting"], [:author_id, "must be set"]], [{error: "no shouting"}, {error: "must be set"}], ["no shouting"], "can't be blank"]"#,
    r#"each side's queries: [true, true, ["Name can't be blank", "Name no shouting"], ["can't be blank"], ["Name no shouting"], true, ["Name can't be blank"], ["no shouting"], ["Author must be set", "Name can't be blank"]]"#,
];

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_errors_surface\n";

/// The port's messages translate through I18n, which the lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const FILES: [(&str, &str); 13] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("app/services/check_base.rb", CHECK_BASE),
    ("app/services/port_check.rb", PORT_CHECK),
    ("app/models/port_probe.rb", PORT_PROBE),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/post.rb", POST),
    ("app/models/entry.rb", ENTRY),
    ("app/models/post_check.rb", POST_CHECK),
    ("app/models/surface_probe.rb", PROBE),
];

const LOADS: [&str; 8] = [
    "app/models/application_record",
    "app/models/post",
    "app/models/entry",
    "app/models/post_check",
    "app/services/check_base",
    "app/services/port_check",
    "app/models/surface_probe",
    "app/models/port_probe",
];

/// Natively and emitted, on the same PostgreSQL, with PortCheck over the
/// ActiveModel::Validations port.
#[test]
#[ignore = "fork spec: postgresql"]
fn errors_surface_matches_native_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    // Library classes emit beside the models.
    let check = std::fs::read_to_string(emitted.join("app/models/port_check.rb")).unwrap();
    assert!(check.contains("include ActiveModel::Validations"), "PortCheck is over the port:\n{check}");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("errors-surface", &FILES, &emitted, "rh_spec_errors_surface", "SurfaceProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("errors-surface-port", &FILES, &emitted, "rh_spec_errors_surface_port", "PortProbe", &LOADS) else { return };
    assert_eq!(native, PORT_EXPECTED, "native, port");
    assert_eq!(emitted_lines, native, "emitted, port");
}

/// The blog's form iterates `article.errors.each do |error|`, which every
/// target answers from the record's `error_details` (the `ErrorDetail`
/// reads `full_message` itself): no tree is refused the port for it, and
/// the CRuby tree's form does not route it over the Errors.
#[test]
fn the_blogs_errors_each_needs_no_port() {
    for target in [BuildTarget::Rust, BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, errors) = emit_and_run::real_blog().emit(target);
        let refused: Vec<_> = errors.iter().filter(|e| e.contains("ErrorAttributes")).collect();
        assert!(refused.is_empty(), "{target:?}: {refused:#?}");
        if target == BuildTarget::Ruby {
            let form = std::fs::read_to_string(emitted.join("app/views/articles/_form.rb")).unwrap();
            assert!(form.contains("article.error_details.each do |error|"), "{form}");
            assert!(!form.contains("ErrorAttributes"), "{form}");
        }
    }
}
