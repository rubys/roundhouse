//! PostgreSQL: errors hash port, on the Ruby target against native Rails.
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
  create_table "authors", force: :cascade do |t|
    t.string "name"
  end
  create_table "posts", force: :cascade do |t|
    t.string "title"
    t.integer "author_id"
  end
  create_table "comments", force: :cascade do |t|
    t.string "body"
    t.integer "post_id"
  end
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE authors (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT authors_pkey PRIMARY KEY (id)
);

CREATE TABLE posts (
    id bigserial NOT NULL,
    title character varying,
    author_id bigint,
    CONSTRAINT posts_pkey PRIMARY KEY (id)
);

CREATE TABLE comments (
    id bigserial NOT NULL,
    body character varying,
    post_id bigint,
    CONSTRAINT comments_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const AUTHOR: &str = "class Author < ApplicationRecord\n  has_many :posts\nend\n";
const COMMENT: &str = "class Comment < ApplicationRecord\n  belongs_to :post, optional: true\n  validates :body, presence: true\nend\n";

/// Each way a message reaches the accumulator: a `validates` check (a
/// literal, a count), `errors.add` with a String, an error type, a
/// `message:` built at run time, on `:base`, an attribute in a local (on
/// `:base` too), and a has_many's own `:invalid`.
const POST: &str = r#"class Post < ApplicationRecord
  belongs_to :author, optional: true
  has_many :comments
  validates :title, presence: true
  validates :title, length: { maximum: 12 }
  validate :checks

  def checks
    errors.add(:base, "no shouting") if title == "LOUD"
    errors.add(:author_id, "must be set") if title == "LOUD"
    errors.add(:title, :invalid) if title == "BAD"
    errors.add(:title, message: "is odd #{title.length}") if title == "ODD"
    flag(:author_id) if title == "FLAG"
    flag("base") if title == "BASE"
  end

  def flag(attribute)
    errors.add(attribute, "is flagged")
  end
end
"#;

/// App code, so the reads go through the compiler. Each line is one
/// answer, or the error it raised.
const PROBE: &str = r##"class HashProbe
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

  def self.lines
    out = []
    show(out, "blank") { invalid("").errors.to_hash }
    show(out, "blank full") { invalid("").errors.to_hash(true) }
    show(out, "too long") { invalid("a title that is too long").errors.as_json }
    show(out, "base and another") { invalid("LOUD").errors.as_json }
    show(out, "base and another full") { invalid("LOUD").errors.as_json(full_messages: true) }
    show(out, "messages") { invalid("LOUD").errors.messages }
    show(out, "messages frozen") { invalid("LOUD").errors.messages.frozen? }
    show(out, "messages missing") { invalid("LOUD").errors.messages[:nothing] }
    show(out, "error type") { invalid("BAD").errors.to_hash }
    show(out, "message option") { invalid("ODD").errors.to_hash }
    show(out, "local attribute") { invalid("FLAG").errors.as_json }
    show(out, "local base full") { invalid("BASE").errors.to_hash(true) }
    show(out, "local base") { invalid("BASE").errors.to_hash }
    show(out, "valid") { invalid("fine").errors.to_hash }
    show(out, "slice") { invalid("LOUD").errors.to_hash.slice(:author_id, :title) }
    show(out, "cleared") do
      post = invalid("")
      post.errors.clear
      post.errors.as_json
    end
    show(out, "revalidated") do
      post = invalid("")
      post.title = "LOUD"
      post.valid?
      post.errors.to_hash
    end
    show(out, "has_many") do
      post = Post.new(title: "fine")
      post.comments.build(body: "")
      post.valid?
      [post.errors.to_hash, post.errors.as_json(full_messages: true)]
    end
    out
  end
end
"##;

/// Rails' answers, spelled out so a run that drifts from native in the
/// same way on both sides still fails.
const EXPECTED: &[&str] = &[
    r#"blank: {title: ["can't be blank"]}"#,
    r#"blank full: {title: ["Title can't be blank"]}"#,
    r#"too long: {title: ["is too long (maximum is 12 characters)"]}"#,
    r#"base and another: {base: ["no shouting"], author_id: ["must be set"]}"#,
    r#"base and another full: {base: ["no shouting"], author_id: ["Author must be set"]}"#,
    r#"messages: {base: ["no shouting"], author_id: ["must be set"]}"#,
    r#"messages frozen: true"#,
    r#"messages missing: []"#,
    r#"error type: {title: ["is invalid"]}"#,
    r#"message option: {title: ["is odd 3"]}"#,
    r#"local attribute: {author_id: ["is flagged"]}"#,
    r#"local base full: {base: ["is flagged"]}"#,
    r#"local base: {base: ["is flagged"]}"#,
    r#"valid: {}"#,
    r#"slice: {author_id: ["must be set"]}"#,
    r#"cleared: {}"#,
    r#"revalidated: {base: ["no shouting"], author_id: ["must be set"]}"#,
    r#"has_many: [{comments: ["is invalid"]}, {comments: ["Comments is invalid"]}]"#,
];

const POST_TEST: &str = r#"require "test_helper"

class PostTest < ActiveSupport::TestCase
  test "errors as json" do
    post = Post.new(title: "LOUD")
    assert_not post.valid?
    assert_equal({ base: ["no shouting"], author_id: ["must be set"] }, post.errors.as_json)
    assert_equal({ base: ["no shouting"], author_id: ["Author must be set"] }, post.errors.to_hash(true))
    post.title = ""
    post.valid?
    assert_equal({ title: ["can't be blank"] }, post.errors.messages)
  end
end
"#;

const MODELS: &[(&str, &str)] = &[
    ("application_record", APPLICATION_RECORD),
    ("author", AUTHOR),
    ("comment", COMMENT),
    ("post", POST),
];

fn expected() -> String {
    EXPECTED.iter().map(|l| format!("{l}\n")).collect()
}

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/hash_probe.rb", PROBE);
    for (name, source) in MODELS {
        app = app.write(&format!("app/models/{name}.rb"), source);
    }
    app
}

fn native() -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_errors_hash_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("schema.rb"), SCHEMA).unwrap();
    std::fs::write(dir.join("hash_probe.rb"), PROBE).unwrap();
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
         {requires}require_relative \"hash_probe\"\n\
         puts HashProbe.lines\n"
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
fn errors_hashes_run_like_rails() {
    let run = app().run_ruby("puts HashProbe.lines");
    run.assert_passes();
    let probe = std::fs::read_to_string(run.emitted.join("app/models/hash_probe.rb")).unwrap();
    assert!(probe.contains("ActiveModel::ErrorAttributes.to_hash(invalid(\"\").errors, true)"), "{probe}");
    let post = std::fs::read_to_string(run.emitted.join("app/models/post.rb")).unwrap();
    // The lowering states each short message's start; none is unsplit.
    // A String message is its own type (`expr::ErrorDetail::Message`).
    assert!(
        post.contains(r#"ActiveModel::ErrorAttributes.add(self.errors, :author_id, "Author must be set", "Author ", ActiveModel::ErrorAttributes::SHORT, {})"#),
        "{post}"
    );
    assert!(post.contains("ActiveModel::ErrorAttributes.add_humanized(self.errors, attribute, "), "{post}");
    assert_eq!(run.stdout, expected(), "emitted vs Rails' answers");
    assert_eq!(run.stdout, native(), "emitted vs native activerecord");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_authored_test_reads_them() {
    let run = app().write("test/models/post_test.rb", POST_TEST).run_test("test/models/post_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("PostTest: 1 tests passed"), "{}", run.stdout);
}

/// A message pushed without its attribute (Rails' collection has no `<<`)
/// is never keyed under a guess: `to_hash` answers from the records each
/// push kept beside the messages (`error_details`), so it names none for
/// it. (The ErrorAttributes route raised naming the message; the records
/// the other trees answer from cannot see a push that kept none.)
#[test]
#[ignore = "fork spec: postgresql"]
fn an_unrecorded_message_is_not_keyed() {
    let run = app()
        .write(
            "app/models/stray.rb",
            "class Stray\n  def self.lines\n    post = Post.new(title: \"\")\n    post.valid?\n    post.errors << \"Hand written\"\n    \
             post.errors.to_hash\n  end\nend\n",
        )
        .run_ruby("puts Stray.lines");
    run.assert_passes();
    assert_eq!(run.stdout.trim(), r#"{title: ["can't be blank"]}"#);
}

/// Spinel emits no port and says so for each read the records beside the
/// messages cannot answer: `to_hash(true)` and `as_json(options)` (their
/// plain forms, and `messages`, are answered from `error_details`, as the
/// grounded `errors.details` is: `lower::errors_index`).
#[test]
#[ignore = "fork spec: postgresql"]
fn spinel_refuses_the_reads() {
    let (dir, errors) = app().emit(BuildTarget::Spinel);
    for read in ["ActiveModel::Errors#to_hash", "ActiveModel::Errors#as_json"] {
        assert!(
            errors.iter().any(|e| e.contains(read)
                && e.contains("declared by the Ruby-family runtime (runtime/active_model_errors.rb), which spinel does not emit")),
            "spinel {read}: {errors:#?}"
        );
    }
    assert!(
        !errors.iter().any(|e| e.contains("ActiveModel::Errors#messages")),
        "`messages` is answered from error_details on spinel: {errors:#?}"
    );
    assert!(!dir.join("runtime/active_model_errors.rb").exists());
}

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_errors_hash\n";

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/author.rb", AUTHOR),
    ("app/models/comment.rb", COMMENT),
    ("app/models/post.rb", POST),
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/author",
    "app/models/comment",
    "app/models/post",
    "app/models/hash_probe",
];

/// Natively and emitted, on the same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn errors_hashes_match_native_rails_on_postgresql() {
    let mut files = FILES.to_vec();
    files.push(("app/models/hash_probe.rb", PROBE));
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("errors-hash", &files, &emitted, "rh_spec_errors_hash", "HashProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
