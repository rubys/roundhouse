//! A uuid-keyed app, emitted to Ruby and RUN on CRuby (#90).
//!
//! `fixtures/tiny-blog-uuid` is tiny-blog with `posts.id` a uuid and
//! `comments.post_id` a uuid foreign key — the shape a Postgres app
//! has by default and no other fixture reaches. The emitted tree is
//! driven through create (a minted key and a supplied one), `find`,
//! `exists?`, update, `has_many`/`belongs_to` across the uuid foreign
//! key, `count`, `all`, and destroy. Unit tests pin what the lowerer
//! WRITES (`tests/primary_key_type.rs`); this pins what it DOES.
//!
//! `#[ignore]`: needs Ruby and the scaffold's gems. CI runs it in the
//! framework-tests-ruby job:
//!
//!     cargo test --test uuid_key_ruby -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

const ORACLE: &str = r#"
require_relative "main"
ENV["BLOG_DB"] = ":memory:"
Main.configure_default_adapter!

p1 = Post.new(title: "first")
raise "unsaved sentinel should be blank, got #{p1.id.inspect}" unless p1.id == ""
p1.save
raise "no uuid minted: #{p1.id.inspect}" unless p1.id =~ /\A[0-9a-f-]{36}\z/

p2 = Post.new(id: "00000000-0000-4000-8000-000000000002", title: "second")
p2.save
raise "supplied key not kept" unless p2.id == "00000000-0000-4000-8000-000000000002"

found = Post.find(p1.id)
raise "find by uuid failed" unless found && found.id == p1.id && found.title == "first"
raise "exists? by uuid failed" unless Post.exists?(p1.id)
raise "find of an absent uuid must raise" unless (Post.find("no-such-key") rescue :raised) == :raised

found.title = "renamed"
found.save
raise "update by uuid failed" unless Post.find(p1.id).title == "renamed"

c = Comment.new(body: "hi")
c.post = p1
raise "belongs_to writer stored #{c.post_id.inspect}" unless c.post_id == p1.id
c.save
raise "comment keeps an integer rowid" unless c.id.is_a?(Integer) && c.id > 0
raise "has_many across a uuid fk failed" unless Post.find(p1.id).comments.map(&:body) == ["hi"]
raise "belongs_to across a uuid fk failed" unless c.post.id == p1.id
c.post = nil
raise "nil writer must blank a uuid fk, got #{c.post_id.inspect}" unless c.post_id == ""
raise "blank uuid fk must read as no row" unless c.post.nil?

raise "count" unless Post.count == 2
raise "all" unless Post.all.map(&:id).sort == [p1.id, p2.id].sort
found.destroy
raise "destroy by uuid failed" if Post.exists?(p1.id)
raise "count after destroy" unless Post.count == 1
puts "UUID KEY OK"
"#;

/// `includes(:post)` across the uuid foreign key. The batch loader names
/// the keys as quoted literals; it used to `to_i` them, so `IN (0)` found
/// nothing and every preloaded post read as nil.
const PRELOAD_ORACLE: &str = r#"
require_relative "main"
ENV["BLOG_DB"] = ":memory:"
Main.configure_default_adapter!

p1 = Post.new(title: "first")
p1.save
p2 = Post.new(title: "second")
p2.save
c1 = Comment.new(body: "on first")
c1.post = p1
c1.save
c2 = Comment.new(body: "on second")
c2.post = p2
c2.save
loaded = ActiveRecord::Relation.new(Comment).includes(:post).to_a
raise "includes(:post) loaded #{loaded.size} comments" unless loaded.size == 2
loaded.each do |c|
  raise "comment #{c.body} preloaded #{c.post.inspect}" unless c.post && c.post.id == c.post_id
end
raise "wrong owner" unless loaded.find { |c| c.id == c1.id }.post.title == "first"
puts "UUID PRELOAD OK"
"#;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// Emit `fixture` to Ruby in `scratch`, then run `oracle` there on CRuby
/// and expect it to print `ok`.
fn emit_and_run_oracle(fixture: &Path, scratch: &Path, oracle: &str, ok: &str) {
    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, fixture, BuildTarget::Ruby)
    });
    let files = files.expect("ruby target files");
    assert!(
        !diags.iter().any(|d| d.message.contains("non_integer_primary_key")),
        "the ruby emit must not ledger the key: {diags:?}"
    );
    roundhouse::project::write_to_dir(&files, scratch).expect("write tree");
    std::fs::write(scratch.join("uuid_oracle.rb"), oracle).expect("write oracle");

    let gemfile = std::fs::canonicalize("runtime/spinel/scaffold/Gemfile").expect("scaffold Gemfile");
    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", gemfile)
        .args(["exec", "ruby", "-I.", "uuid_oracle.rb"])
        .current_dir(scratch)
        .output()
        .expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(ok),
        "uuid oracle failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean scratch");
    }
    std::fs::create_dir_all(&dir).expect("create scratch");
    dir
}

#[test]
#[ignore]
fn a_uuid_belongs_to_preload_loads_its_owners() {
    // The batch loaders are emitted only when the app's own code calls
    // `includes`, so this runs on a copy of the fixture whose Comment
    // model does. (That call is lowered to a plain query at compile
    // time; the oracle drives the runtime Relation, which runs the
    // loaders.)
    let fixture = fresh_dir("roundhouse-uuid-preload-app");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog-uuid"), &fixture);
    let comment = fixture.join("app/models/comment.rb");
    let src = std::fs::read_to_string(&comment).expect("read comment.rb");
    let src = src.replacen(
        "  def self.counts_by_post",
        "  def self.with_posts\n    Comment.includes(:post).to_a\n  end\n\n  def self.counts_by_post",
        1,
    );
    assert!(src.contains("with_posts"), "comment.rb no longer has counts_by_post");
    std::fs::write(&comment, src).expect("write comment.rb");
    emit_and_run_oracle(&fixture, &fresh_dir("roundhouse-uuid-preload-ruby"), PRELOAD_ORACLE, "UUID PRELOAD OK");
}

#[test]
#[ignore]
fn a_uuid_keyed_app_runs_on_cruby() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog-uuid");
    emit_and_run_oracle(&fixture, &fresh_dir("roundhouse-uuid-key-ruby"), ORACLE, "UUID KEY OK");
}
