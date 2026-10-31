//! PostgreSQL: errors association ports, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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
  create_table "covers", force: :cascade do |t|
    t.string "url"
    t.integer "post_id"
  end
end
"#;

const MODELS: &[(&str, &str)] = &[
    ("application_record", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("author", "class Author < ApplicationRecord\n  has_many :posts\nend\n"),
    ("comment", "class Comment < ApplicationRecord\n  belongs_to :post\nend\n"),
    ("cover", "class Cover < ApplicationRecord\n  belongs_to :post\nend\n"),
    (
        "post",
        r#"class Post < ApplicationRecord
  belongs_to :author, optional: true
  has_many :comments
  has_one :cover
  validates :title, presence: true
  validates :title, length: { maximum: 12 }
  validate :title_not_shouting

  def title_not_shouting
    errors.add(:base, "no shouting") if title == "LOUD"
    errors.add(:author_id, "must be set") if title == "LOUD"
    flag(:author_id) if title == "FLAG"
    flag("title") if title == "FLAGS"
  end

  def flag(attribute)
    errors.add(attribute, "is flagged")
  end
end
"#,
    ),
];

/// App code, so both reads go through the compiler. `show` prints one
/// answer, or the error it raised.
const PROBE: &str = r##"module PortsProbe
  def self.show(label)
    value = begin
      yield
    rescue StandardError => e
      [e.class.name, e.message]
    end
    puts "#{label}: #{value.inspect}"
  end

  def self.names_after(attrs)
    post = Post.new(attrs)
    post.valid?
    post.errors.attribute_names
  end

  def self.run
    show("blank title") { names_after(title: "") }
    show("shouting") { names_after(title: "LOUD") }
    show("too long") { names_after(title: "a title that is too long") }
    show("flagged by a local") { names_after(title: "FLAG") }
    show("flagged by a string") { names_after(title: "FLAGS") }
    show("valid") { names_after(title: "fine") }
    show("frozen") { names_after(title: "LOUD").frozen? }
    show("never validated") { Post.new(title: "LOUD").errors.attribute_names }
    show("revalidated") do
      post = Post.new(title: "")
      post.valid?
      post.title = "fine"
      post.valid?
      post.errors.attribute_names
    end
    show("required belongs_to") do
      comment = Comment.new(body: "orphan")
      comment.valid?
      comment.errors.attribute_names
    end
    show("cleared") do
      post = Post.new(title: "LOUD")
      post.valid?
      post.errors.clear
      post.errors.attribute_names
    end

    post = Post.create!(title: "first")
    comment = Comment.create!(body: "a", post_id: post.id)
    Cover.create!(url: "u", post_id: post.id)

    fresh = Post.find(post.id)
    comments = fresh.association(:comments)
    show("has_many class") { comments.class.name }
    show("has_many loaded? before") { comments.loaded? }
    show("has_many target before") { comments.target }
    show("has_many load_target") { comments.load_target.map(&:body) }
    show("has_many loaded? after load_target") { comments.loaded? }
    show("has_many target after") { fresh.association(:comments).target.map(&:body) }
    show("has_many target length") { fresh.association(:comments).target.length }
    show("has_many reset") { comments.reset }
    show("has_many loaded? after reset") { comments.loaded? }
    show("has_many target after reset") { comments.target }
    show("has_many reload") { fresh.association(:comments).reload.class.name }
    show("has_many loaded? after reload") { fresh.association(:comments).loaded? }
    show("by a String name") { Post.find(post.id).association("comments").class.name }

    seeded = Post.find(post.id)
    seeded.association(:comments).target = [comment]
    show("target= loaded?") { seeded.association(:comments).loaded? }
    show("target= read through the reader") { seeded.comments.map(&:body) }

    c = Comment.find(comment.id)
    parent = c.association(:post)
    show("belongs_to class") { parent.class.name }
    show("belongs_to loaded? before") { parent.loaded? }
    show("belongs_to target before") { parent.target }
    show("belongs_to reader read") { c.post.title }
    show("belongs_to loaded? after read") { parent.loaded? }
    show("belongs_to target after read") { parent.target.title }
    show("belongs_to reset") { parent.reset }
    show("belongs_to after reset") { [parent.loaded?, parent.target] }
    show("belongs_to association reader") { parent.reader.title }
    show("belongs_to loaded? after reader") { parent.loaded? }

    h = Post.find(post.id)
    cover = h.association(:cover)
    show("has_one class") { cover.class.name }
    show("has_one before") { [cover.loaded?, cover.target] }
    show("has_one target never read") { Post.find(post.id).association(:cover).target }
    show("has_one reader") { cover.reader.url }
    show("has_one after reader") { [cover.loaded?, cover.target.url] }
    show("has_one reload") { cover.reload.target.url }

    preloaded = Post.includes(:comments, :cover).to_a.first
    show("preloaded has_many") { [preloaded.association(:comments).loaded?, preloaded.association(:comments).target.map(&:body)] }
    show("preloaded has_one") { [preloaded.association(:cover).loaded?, preloaded.association(:cover).target.url] }

    show("another model") { Author.new(name: "x").association(:posts).class.name }
    # Named by a value, not a literal: a literal name no model has is
    # refused at check (`analyze::association_surface`).
    nope = :nope
    show("unknown association") { fresh.association(nope) }
    other = "comments"
    show("unknown on another model") { Cover.new.association(other) }
  end
end
"##;

/// Rails' answers, spelled out so a run that drifts from native in the
/// same way on both sides (an oracle that loads the wrong Rails) still fails.
const EXPECTED: &str = r#"blank title: [:title]
shouting: [:base, :author_id]
too long: [:title]
flagged by a local: [:author_id]
flagged by a string: [:title]
valid: []
frozen: true
never validated: []
revalidated: []
required belongs_to: [:post]
cleared: []
has_many class: "ActiveRecord::Associations::HasManyAssociation"
has_many loaded? before: false
has_many target before: []
has_many load_target: ["a"]
has_many loaded? after load_target: true
has_many target after: ["a"]
has_many target length: 1
has_many reset: nil
has_many loaded? after reset: false
has_many target after reset: []
has_many reload: "ActiveRecord::Associations::HasManyAssociation"
has_many loaded? after reload: true
by a String name: "ActiveRecord::Associations::HasManyAssociation"
target= loaded?: true
target= read through the reader: ["a"]
belongs_to class: "ActiveRecord::Associations::BelongsToAssociation"
belongs_to loaded? before: false
belongs_to target before: nil
belongs_to reader read: "first"
belongs_to loaded? after read: true
belongs_to target after read: "first"
belongs_to reset: false
belongs_to after reset: [false, nil]
belongs_to association reader: "first"
belongs_to loaded? after reader: true
has_one class: "ActiveRecord::Associations::HasOneAssociation"
has_one before: [false, nil]
has_one target never read: nil
has_one reader: "u"
has_one after reader: [true, "u"]
has_one reload: "u"
preloaded has_many: [true, ["a"]]
preloaded has_one: [true, "u"]
another model: "ActiveRecord::Associations::HasManyAssociation"
unknown association: ["ActiveRecord::AssociationNotFoundError", "Association named 'nope' was not found on Post; perhaps you misspelled it?"]
unknown on another model: ["ActiveRecord::AssociationNotFoundError", "Association named 'comments' was not found on Cover; perhaps you misspelled it?"]
"#;

const POST_TEST: &str = r#"require "test_helper"

class PostTest < ActiveSupport::TestCase
  test "attribute_names names each invalid attribute once" do
    post = Post.new(title: "LOUD")
    assert_not post.valid?
    assert_equal [:base, :author_id], post.errors.attribute_names
    post.title = ""
    post.valid?
    assert_equal [:title], post.errors.attribute_names
  end

  test "association answers what is in memory" do
    post = Post.create!(title: "first")
    Comment.create!(body: "a", post_id: post.id)
    record = Post.find(post.id)
    assert_not record.association(:comments).loaded?
    assert_equal [], record.association(:comments).target
    record.association(:comments).load_target
    assert record.association(:comments).loaded?
    assert_equal ["a"], record.association(:comments).target.map(&:body)
    assert_raises(ActiveRecord::AssociationNotFoundError) { record.association(:nope) }
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("lib/ports_probe.rb", PROBE);
    for (name, source) in MODELS {
        app = app.write(&format!("app/models/{name}.rb"), source);
    }
    app
}

fn native() -> String {
    native_run("ports", SCHEMA, MODELS, "ports_probe", PROBE, "PortsProbe")
}

fn native_run(
    tag: &str,
    schema: &str,
    models: &[(&str, &str)],
    probe_file: &str,
    probe: &str,
    module: &str,
) -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_errors_association_native_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("schema.rb"), schema).unwrap();
    std::fs::write(dir.join(format!("{probe_file}.rb")), probe).unwrap();
    let mut requires = String::new();
    for (name, source) in models {
        let path = dir.join(format!("{name}.rb"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
        requires.push_str(&format!("require_relative {name:?}\n"));
    }
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", \"activemodel\", \"activesupport\")\n\
         require \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\n\
         # A Rails 5+ app's defaults (`load_defaults`): a belongs_to is required.\n\
         ActiveRecord::Base.belongs_to_required_by_default = true\n\
         load File.expand_path(\"schema.rb\", __dir__)\n\
         {requires}require_relative {probe_file:?}\n\
         {module}.run\n"
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
fn attribute_names_and_association_run_like_rails() {
    let run = app().run_ruby("PortsProbe.run");
    run.assert_passes();
    let probe = std::fs::read_to_string(run.emitted.join("app/models/ports_probe.rb")).unwrap();
    // The reads are Rails' own, not collapsed onto the reader (which
    // would load where Rails answers what is in memory).
    assert!(probe.contains("fresh.association(:comments).target.length"), "{probe}");
    assert!(probe.contains("Post.find(post.id).association(:cover).target"), "{probe}");
    assert_eq!(run.stdout, EXPECTED, "emitted on the ports vs Rails' answers");
    assert_eq!(run.stdout, native(), "emitted on the ports vs native activerecord");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_authored_test_reads_both() {
    let run = app().write("test/models/post_test.rb", POST_TEST).run_test("test/models/post_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("PostTest: 2 tests passed"), "{}", run.stdout);
}

/// The PostgreSQL CRuby tree carries the association port. `attribute_names`
/// is answered from the record's `error_details` on every tree
/// (`lower::errors_index`), so the ErrorAttributes port is not carried for it.
#[test]
#[ignore = "fork spec: postgresql"]
fn the_postgresql_tree_carries_the_association_port() {
    let (dir, errors) = app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_spec_errors_association\n")
        .emit(BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(dir.join("runtime/active_record_associations.rb").is_file(), "runtime/active_record_associations.rb is carried");
    assert!(!dir.join("runtime/active_model_errors.rb").exists(), "attribute_names needs no ErrorAttributes port");
    let probe = std::fs::read_to_string(dir.join("app/models/ports_probe.rb")).unwrap();
    assert!(probe.contains("ActiveSupport.errors_attribute_names(post.error_details)"), "{probe}");
}

/// Spinel emits neither port. It says so for `association`; the plain
/// `attribute_names` is answered from the record's `error_details`
/// (`lower::errors_index`), as `errors.details` is, so it is not refused.
#[test]
#[ignore = "fork spec: postgresql"]
fn spinel_refuses_both_reads() {
    let (dir, errors) = app().emit(BuildTarget::Spinel);
    let expected = "declared by the Ruby-family runtime (runtime/active_record_associations.rb), which spinel does not emit";
    assert!(
        errors.iter().any(|e| e.contains("#association") && e.contains(expected)),
        "spinel #association: {errors:#?}"
    );
    assert!(
        !errors.iter().any(|e| e.contains("ActiveModel::Errors#attribute_names")),
        "`attribute_names` is answered from error_details on spinel: {errors:#?}"
    );
    for file in ["runtime/active_model_errors.rb", "runtime/active_record_associations.rb"] {
        assert!(!dir.join(file).exists(), "{file} stays off the spinel tree");
    }
}

// ---- The association surface: every association a model has, what a
// read leaves loaded, `klass`, `scope`, `stale_target?`, and the
// CollectionProxy's `reload` / `reset`.

const SURFACE_SCHEMA: &str = r#"ActiveRecord::Schema[8.0].define(version: 1) do
  create_table "authors", force: :cascade do |t|
    t.string "name"
  end
  create_table "posts", force: :cascade do |t|
    t.string "title"
    t.string "type"
    t.integer "author_id"
  end
  create_table "comments", force: :cascade do |t|
    t.string "body"
    t.integer "post_id"
  end
  create_table "notes", force: :cascade do |t|
    t.string "text"
    t.integer "post_id"
  end
  create_table "stars", force: :cascade do |t|
    t.integer "post_id"
    t.string "subject_type"
    t.integer "subject_id"
  end
end
"#;

/// A belongs_to declared on an abstract base (`Authored`), a has_many
/// spliced from a concern (`Annotated`), an STI child (`FeaturedPost`)
/// inheriting both, a polymorphic belongs_to (`Star#subject`) with its
/// `as:` side, and a has_many :through. In the order native requires
/// them.
const SURFACE_MODELS: &[(&str, &str)] = &[
    ("application_record", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("authored", "class Authored < ApplicationRecord\n  self.abstract_class = true\n  belongs_to :author, optional: true\nend\n"),
    ("concerns/annotated", "module Annotated\n  extend ActiveSupport::Concern\n\n  included do\n    has_many :notes\n  end\nend\n"),
    (
        "author",
        "class Author < ApplicationRecord\n  has_many :posts\n  has_many :stars, as: :subject\n  has_many :post_comments, through: :posts, source: :comments\nend\n",
    ),
    ("post", "class Post < Authored\n  include Annotated\n  has_many :comments\nend\n"),
    ("featured_post", "class FeaturedPost < Post\nend\n"),
    ("comment", "class Comment < ApplicationRecord\n  belongs_to :post\nend\n"),
    ("note", "class Note < ApplicationRecord\n  belongs_to :post\nend\n"),
    ("star", "class Star < ApplicationRecord\n  belongs_to :post\n  belongs_to :subject, polymorphic: true, optional: true\nend\n"),
];

const SURFACE_PROBE: &str = r##"module SurfaceProbe
  def self.show(label)
    value = begin
      yield
    rescue StandardError => e
      [e.class.name, e.message]
    end
    puts "#{label}: #{value.inspect}"
  end

  def self.run
    surface
    edges
  end

  def self.surface
    author = Author.create!(name: "a")
    post = FeaturedPost.create!(title: "t", author_id: author.id)
    Comment.create!(body: "c1", post_id: post.id)
    Note.create!(text: "n1", post_id: post.id)

    show("concern has_many") { Post.find(post.id).association(:notes).class.name }
    show("abstract base belongs_to") { Post.find(post.id).association(:author).class.name }

    show("sti child inherited") { FeaturedPost.find(post.id).association(:notes).class.name }

    r = Post.find(post.id)
    show("has_many read then loaded?") { r.comments.map(&:body); r.association(:comments).loaded? }
    show("has_many read then flat loaded?") { r.comments.loaded? }
    show("has_many target after read") { r.association(:comments).target.map(&:body) }
    r2 = Post.find(post.id)
    show("has_many first not loaded") { r2.comments.first.body; r2.association(:comments).loaded? }
    show("has_many count not loaded") { r2.comments.count; r2.association(:comments).loaded? }
    show("has_many size not loaded") { r2.comments.size; r2.association(:comments).loaded? }
    show("has_many empty? not loaded") { r2.comments.empty?; r2.association(:comments).loaded? }
    r3 = Post.find(post.id)
    show("loaded then create! through") { r3.comments.to_a; r3.comments.create!(body: "c2"); r3.comments.map(&:body) }
    show("loaded then create! loaded?") { r3.association(:comments).loaded? }
    show("loaded then outside create") { Comment.create!(body: "c3", post_id: post.id); r3.comments.map(&:body) }
    show("reload sees all") { r3.comments.reload.map(&:body) }
    a = Author.find(author.id)
    show("belongs_to read then loaded?") { Post.find(post.id).tap { |p| p.author.name }.association(:author).loaded? }
    show("has_many through author") { a.posts.map(&:title); a.association(:posts).loaded? }

    r4 = Post.find(post.id)
    show("reset after read") { r4.comments.to_a; r4.comments.reset; r4.association(:comments).loaded? }
    show("read after reset") { r4.comments.map(&:body) }

    show("klass has_many") { r4.association(:comments).klass.name }
    show("klass belongs_to inherited") { r4.association(:author).klass.name }
    show("klass concern") { r4.association(:notes).klass.name }
    show("scope has_many") { r4.association(:comments).scope.to_a.map(&:body).sort }
    show("scope has_many count") { r4.association(:comments).scope.count }
    show("scope belongs_to pick") { r4.association(:author).scope.pick(:name) }
    show("scope concern") { r4.association(:notes).scope.pick(:text) }
    star = Star.create!(post_id: post.id, subject_type: "Author", subject_id: author.id)
    show("klass polymorphic") { Star.find(star.id).association(:subject).klass.name }
    show("klass polymorphic nil") { Star.new.association(:subject).klass }
    show("scope polymorphic") { Star.find(star.id).association(:subject).scope.pick(:name) }

    other = Author.create!(name: "b")
    p1 = Post.find(post.id)
    show("stale before read") { p1.association(:author).stale_target? }
    show("stale after read") { p1.author.name; p1.association(:author).stale_target? }
    show("fk write: loaded?") { p1.author_id = other.id; p1.association(:author).loaded? }
    show("fk write: stale?") { p1.association(:author).stale_target? }
    show("fk write: target") { p1.association(:author).target.name }
    show("fk write: reader") { p1.author.name }
    show("after re-read: stale?") { p1.association(:author).stale_target? }
    p2 = Post.find(post.id)
    show("fk write back: stale?") { p2.author.name; p2.author_id = other.id; p2.author_id = author.id; p2.association(:author).stale_target? }
    show("fk write back: loaded?") { p2.association(:author).loaded? }
    p3 = Post.find(post.id)
    show("fk write unloaded: loaded?") { p3.author_id = other.id; p3.association(:author).loaded? }
    show("fk write unloaded: stale?") { p3.association(:author).stale_target? }
    show("writer assign: stale?") { p3.author = author; p3.association(:author).stale_target? }
    show("has_many stale?") { r4.association(:comments).stale_target? }
    show("reset clears stale") { p1.author_id = author.id; p1.association(:author).reset; [p1.association(:author).loaded?, p1.association(:author).stale_target?] }
  end

  # Through, polymorphic and as: reads, and writer assignment.
  def self.edges
    author = Author.create!(name: "a")
    post = Post.create!(title: "t", author_id: author.id)
    Comment.create!(body: "c1", post_id: post.id)
    a = Author.find(author.id)
    show("through before") { a.association(:post_comments).loaded? }
    show("through read") { a.post_comments.map(&:body) }
    show("through after read") { a.association(:post_comments).loaded? }
    show("through flat") { a.post_comments.loaded? }
    show("through target") { a.association(:post_comments).target.map(&:body) }
    b = Author.find(author.id)
    show("through load_target") { b.association(:post_comments).load_target.map(&:body) }
    show("through loaded after load_target") { [b.association(:post_comments).loaded?, b.post_comments.loaded?] }
    s = Star.create!(post_id: post.id, subject_type: "Author", subject_id: author.id)
    st = Star.find(s.id)
    show("poly before") { st.association(:subject).loaded? }
    show("poly read") { st.subject.name }
    show("poly after read") { st.association(:subject).loaded? }
    au = Author.find(author.id)
    show("as: read") { au.stars.map(&:id).size }
    show("as: after") { au.association(:stars).loaded? }
    pp = Post.find(post.id)
    show("belongs_to read loaded") { pp.author.name; pp.association(:author).loaded? }
    other = Author.create!(name: "b")
    show("belongs_to assign then read") { pp.author = other; pp.author.name }
    show("belongs_to assign loaded/stale") { [pp.association(:author).loaded?, pp.association(:author).stale_target?] }
    show("belongs_to assign nil") { pp.author = nil; [pp.author, pp.association(:author).loaded?] }
    show("poly assign then read") { st.subject = other; st.subject.name }
    st2 = Star.find(s.id)
    show("poly fk write") { st2.subject.name; st2.subject_id = other.id; [st2.association(:subject).loaded?, st2.subject.name] }
    show("poly klass after type write") { st2.subject_type = "Post"; st2.association(:subject).klass.name }
    a2 = Author.find(author.id)
    show("through chained where not loaded") { a2.post_comments.where(body: "c1").to_a.size; a2.association(:post_comments).loaded? }
    show("through first not loaded") { a2.post_comments.first.body; a2.association(:post_comments).loaded? }
    show("through each loads") { a2.post_comments.each { |c| c.body }; a2.association(:post_comments).loaded? }
    show("through reload") { Comment.create!(body: "c2", post_id: post.id); a2.post_comments.reload.map(&:body) }
    show("through reload loaded") { a2.association(:post_comments).target.map(&:body) }
  end
end
"##;

/// Native activerecord's answers, spelled out.
const SURFACE_EXPECTED: &str = r#"concern has_many: "ActiveRecord::Associations::HasManyAssociation"
abstract base belongs_to: "ActiveRecord::Associations::BelongsToAssociation"
sti child inherited: "ActiveRecord::Associations::HasManyAssociation"
has_many read then loaded?: true
has_many read then flat loaded?: true
has_many target after read: ["c1"]
has_many first not loaded: false
has_many count not loaded: false
has_many size not loaded: false
has_many empty? not loaded: false
loaded then create! through: ["c1", "c2"]
loaded then create! loaded?: true
loaded then outside create: ["c1", "c2"]
reload sees all: ["c1", "c2", "c3"]
belongs_to read then loaded?: true
has_many through author: true
reset after read: false
read after reset: ["c1", "c2", "c3"]
klass has_many: "Comment"
klass belongs_to inherited: "Author"
klass concern: "Note"
scope has_many: ["c1", "c2", "c3"]
scope has_many count: 3
scope belongs_to pick: "a"
scope concern: "n1"
klass polymorphic: "Author"
klass polymorphic nil: nil
scope polymorphic: "a"
stale before read: false
stale after read: false
fk write: loaded?: true
fk write: stale?: true
fk write: target: "a"
fk write: reader: "b"
after re-read: stale?: false
fk write back: stale?: false
fk write back: loaded?: true
fk write unloaded: loaded?: false
fk write unloaded: stale?: false
writer assign: stale?: false
has_many stale?: false
reset clears stale: [false, false]
through before: false
through read: ["c1"]
through after read: true
through flat: true
through target: ["c1"]
through load_target: ["c1"]
through loaded after load_target: [true, true]
poly before: false
poly read: "a"
poly after read: true
as: read: 1
as: after: true
belongs_to read loaded: true
belongs_to assign then read: "b"
belongs_to assign loaded/stale: [true, false]
belongs_to assign nil: [nil, true]
poly assign then read: "b"
poly fk write: [true, "b"]
poly klass after type write: "Post"
through chained where not loaded: false
through first not loaded: false
through each loads: true
through reload: ["c1", "c2"]
through reload loaded: ["c1", "c2"]
"#;

/// An authored test over the same surface.
const SURFACE_TEST: &str = r#"require "test_helper"

class PostTest < ActiveSupport::TestCase
  test "the association answers as Rails' does" do
    author = Author.create!(name: "a")
    other = Author.create!(name: "b")
    post = Post.create!(title: "t", author_id: author.id)
    Comment.create!(body: "c1", post_id: post.id)
    record = Post.find(post.id)
    assert_equal Comment, record.association(:comments).klass
    assert_equal ["c1"], record.association(:comments).scope.to_a.map(&:body)
    assert_equal "a", record.association(:author).scope.pick(:name)
    record.comments.map(&:body)
    assert record.association(:comments).loaded?
    Comment.create!(body: "c2", post_id: post.id)
    assert_equal ["c1", "c2"], record.comments.reload.map(&:body)
    assert_equal "a", record.author.name
    record.author_id = other.id
    assert record.association(:author).loaded?
    assert record.association(:author).stale_target?
    assert_equal "b", record.author.name
    assert_not record.association(:author).stale_target?
    assert_raises(ActiveRecord::AssociationNotFoundError) { record.association(:nope) }
  end
end
"#;

fn surface_app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SURFACE_SCHEMA)
        .write("lib/surface_probe.rb", SURFACE_PROBE);
    for (name, source) in SURFACE_MODELS {
        app = app.write(&format!("app/models/{name}.rb"), source);
    }
    app
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_association_surface_runs_like_rails() {
    let run = surface_app().run_ruby("SurfaceProbe.run");
    run.assert_passes();
    let probe = std::fs::read_to_string(run.emitted.join("app/models/surface_probe.rb")).unwrap();
    // The proxy's reload and reset run on the association object.
    assert!(probe.contains("ActiveRecord::Associations::Association.proxy(r3, :comments, :reload)"), "{probe}");
    assert!(probe.contains("ActiveRecord::Associations::Association.proxy(r4, :comments, :reset)"), "{probe}");
    let author = std::fs::read_to_string(run.emitted.join("app/models/author.rb")).unwrap();
    assert!(author.contains("ActiveRecord::Associations::Association.through(self, :post_comments, "), "{author}");
    assert_eq!(run.stdout, SURFACE_EXPECTED, "emitted vs Rails' answers");
    assert_eq!(
        run.stdout,
        native_run("surface", SURFACE_SCHEMA, SURFACE_MODELS, "surface_probe", SURFACE_PROBE, "SurfaceProbe"),
        "emitted vs native activerecord"
    );
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_authored_test_reads_the_surface() {
    let run = surface_app().write("test/models/post_test.rb", SURFACE_TEST).run_test("test/models/post_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("PostTest: 1 tests passed") || run.stdout.contains("PostTest: 1 test passed"), "{}", run.stdout);
}

/// What the object cannot answer as Rails does is refused at check, by
/// name; an authored `assert_raises(ActiveRecord::AssociationNotFoundError)`
/// around an unknown name is not (the emitted record raises it).
#[test]
#[ignore = "fork spec: postgresql"]
fn check_refuses_what_the_association_cannot_answer() {
    let refused = r#"module Refused
  def self.run
    post = Post.first
    post.association(:nope)
    post.association(:image_attachment)
    Author.first.association(:post_comments).stale_target?
    Star.first.association(:subject).stale_target?
    post.comments.reset.size
  end
end
"#;
    let (_dir, errors) = surface_app()
        .write("lib/refused.rb", refused)
        .write("test/models/post_test.rb", SURFACE_TEST)
        .emit(BuildTarget::Ruby);
    for expected in [
        "`association(:nope)` on Post",
        "`association(:image_attachment)` on Post",
        "`association(:post_comments).stale_target?` on Author: not ported for a :through association",
        "`association(:subject).stale_target?` on Star: not ported for a polymorphic belongs_to",
        "`comments.reset.size`",
    ] {
        assert!(errors.iter().any(|e| e.contains(expected)), "{expected}: {errors:#?}");
    }
    assert_eq!(errors.len(), 5, "only the five, not the test's assert_raises: {errors:#?}");
}
