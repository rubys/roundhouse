//! A nested record's validation reaching its owner (#1335, part 4).
//! activerecord 8.1.4 (oracle `nested_validation.rb`):
//! - `accepts_nested_attributes_for` makes the association autosave: a
//!   loaded child edited to invalid fails the owner's `save`, its error
//!   under the association ("Comments body can't be blank"), with nothing
//!   written; one edited validly is saved with the owner. `has_many …,
//!   autosave: true` does the same without nested attributes.
//! - The association's check runs where it's declared, so its errors list
//!   before a later `validates`'s.
//! - Two children with the same error add it to the owner once.
//!
//! Before: an association reader re-queried on every call, so the owner
//! never saw an edited child and saved without it; the check ran after
//! every `validates`; and each child added its own copy of an error.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const BODY: &str = "A body long enough to validate.";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
body = "A body long enough to validate."
both = Article.new(title: "", body: body, comments_attributes: [{ commenter: "c", body: "" }])
both.valid?
check("the association's errors list first", both.errors, ["Comments body can't be blank", "Title can't be blank"])
twice = Article.new(title: "t", body: body, comments_attributes: [{ commenter: "c", body: "" }, { commenter: "d", body: "" }])
twice.valid?
check("the same error once", twice.errors, ["Comments body can't be blank"])
check("one detail", twice.error_details.map(&:attribute), ["comments.body"])
a = Article.create!(title: "t", body: body, comments_attributes: [{ commenter: "c", body: "one" }])
id = Comment.where(article_id: a.id).first.id
q = Article.find(a.id)
q.comments.to_a.first.body = ""
check("a loaded child edited to invalid fails the owner", q.save, false)
check("its error under the association", q.errors, ["Comments body can't be blank"])
check("nothing written", Comment.find(id).body, "one")
r = Article.find(a.id)
r.comments.to_a.first.body = "two"
check("one edited validly saves", r.save, true)
check("with the owner", Comment.find(id).body, "two")
check("an unloaded association is left alone", Article.find(a.id).save, true)
puts "nested validation passed"
"##;

fn nested_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
        "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments\n",
    )
}

#[test]
fn a_nested_childs_validation_reaches_its_owner_on_ruby() {
    let run = nested_app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested validation passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_nested_childs_validation_reaches_its_owner_on_spinel() {
    let run = nested_app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested validation passed"), "{}", run.stdout);
}

const AUTOSAVE_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = Article.create!(title: "t", body: "BODY")
c = Comment.create!(article_id: a.id, commenter: "c", body: "one")
q = Article.find(a.id)
q.comments.to_a.first.body = ""
check("autosave: an invalid loaded child fails the owner", q.save, false)
check("its error", q.errors, ["Comments body can't be blank"])
check("nothing written", Comment.find(c.id).body, "one")
r = Article.find(a.id)
r.comments.to_a.first.body = "two"
r.save
check("autosave: a valid edit is saved", Comment.find(c.id).body, "two")
puts "autosave passed"
"##;

#[test]
fn has_many_autosave_validates_and_saves_loaded_children_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy, autosave: true\n",
        )
        .run_ruby(&AUTOSAVE_SCRIPT.replace("BODY", BODY));
    run.assert_passes();
    assert!(run.stdout.contains("autosave passed"), "{}", run.stdout);
}

// Association options that change what Rails does and that aren't
// modelled are reported, not dropped silently: `index_errors:` (Rails
// prefixes "Comments[1] body …") and `counter_cache:` here.
#[test]
fn an_ignored_association_option_is_reported() {
    let root = std::env::temp_dir().join(format!("roundhouse-ignored-assoc-option-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let edit = |path: &str, from: &str, to: &str| {
        let text = std::fs::read_to_string(source.join(path)).unwrap().replacen(from, to, 1);
        std::fs::write(source.join(path), text).unwrap();
    };
    edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", "  has_many :comments, dependent: :destroy, index_errors: true\n");
    edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, counter_cache: true\n");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reported: Vec<&str> = stderr.lines().filter(|l| l.contains("association option is not modelled")).collect();
    assert_eq!(reported.len(), 2, "{stderr}");
    assert!(reported.iter().any(|l| l.contains("has_many :comments, index_errors:")), "{reported:?}");
    assert!(reported.iter().any(|l| l.contains("belongs_to :article, counter_cache:")), "{reported:?}");
}

// A loaded child that nested attributes also stage (oracle
// `cr_oracle.rb`): marked `_destroy`, it isn't validated or saved but
// destroyed, and the owner saves; updated, its error reaches the owner
// once, from the nested check alone.
const STAGED_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
body = "A body long enough to validate."
a = Article.create!(title: "t", body: body, comments_attributes: [{ commenter: "c", body: "one" }])
q = Article.find(a.id)
c = q.comments.to_a.first
c.body = ""
q.comments_attributes = [{ id: c.id, _destroy: "1" }]
check("a loaded child marked for destruction isn't validated", [q.save, q.errors], [true, []])
check("it's destroyed", Comment.exists?(c.id), false)
b = Article.create!(title: "t", body: body, comments_attributes: [{ commenter: "c", body: "one" }])
r = Article.find(b.id)
d = r.comments.to_a.first
r.comments_attributes = [{ id: d.id, body: "" }]
check("a loaded child the writer updates: its error once", [r.save, r.errors], [false, ["Comments body can't be blank"]])
puts "staged children passed"
"##;

#[test]
fn a_loaded_child_staged_by_nested_attributes_is_checked_once_on_ruby() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  accepts_nested_attributes_for :comments, allow_destroy: true\n",
        )
        .run_ruby(STAGED_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("staged children passed"), "{}", run.stdout);
}
