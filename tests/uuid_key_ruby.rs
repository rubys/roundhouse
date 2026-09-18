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

#[test]
#[ignore]
fn a_uuid_keyed_app_runs_on_cruby() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog-uuid");
    let scratch: PathBuf = std::env::temp_dir().join("roundhouse-uuid-key-ruby");
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let mut app = ingest_app(&fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, &fixture, BuildTarget::Ruby)
    });
    let files = files.expect("ruby target files");
    assert!(
        !diags.iter().any(|d| d.message.contains("non_integer_primary_key")),
        "the ruby emit must not ledger the key: {diags:?}"
    );
    roundhouse::project::write_to_dir(&files, &scratch).expect("write tree");
    std::fs::write(scratch.join("uuid_oracle.rb"), ORACLE).expect("write oracle");

    let gemfile = std::fs::canonicalize("runtime/spinel/scaffold/Gemfile").expect("scaffold Gemfile");
    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", gemfile)
        .args(["exec", "ruby", "-I.", "uuid_oracle.rb"])
        .current_dir(&scratch)
        .output()
        .expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("UUID KEY OK"),
        "uuid oracle failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
