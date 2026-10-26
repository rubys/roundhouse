use super::emit_and_run;

/// #1214: assigning nil to a `belongs_to` stores nil in its foreign key,
/// as Rails 8.1.4 does, on the ruby family. The writer stored the key's
/// sentinel (`0`), so `author_id.nil?` was false, `presence:` on the key
/// passed, the row saved `0`, and `where(author_id: nil)` missed it.
/// Answers are Rails 8.1.4's for the same models and records.
fn app(post_model: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/author.rb", "class Author < ApplicationRecord\nend\n")
        .write("app/models/comment.rb", "class Comment < ApplicationRecord\n  has_many :pins, as: :subject\nend\n")
        .write("app/models/pin.rb", "class Pin < ApplicationRecord\n  belongs_to :subject, polymorphic: true, optional: true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"authors\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n    t.integer \"author_id\"\n  end\n  create_table \"comments\", force: :cascade do |t|\n    t.string \"body\"\n  end\n  create_table \"pins\", force: :cascade do |t|\n    t.string \"subject_type\"\n    t.integer \"subject_id\"\n  end\nend\n",
        )
        .write("app/models/post.rb", post_model)
}

const SETUP: &str = r#"a = Author.new
a.name = "a"
a.save!
fresh = Post.new
fresh.title = "fresh"
assigned = Post.new
assigned.title = "assigned nil"
assigned.author = nil
cleared = Post.new
cleared.title = "cleared"
cleared.author = a
cleared.author = nil
"#;

#[test]
fn a_nil_belongs_to_reads_and_validates_as_nil_like_rails() {
    let script = format!(
        "{SETUP}{}",
        r#"got = [fresh, assigned, cleared].map do |post|
  [post.author_id, post.author_id.nil?, post.author.nil?, post.valid?, post.errors.to_a]
end
want = [
  [nil, true, true, false, ["Author can't be blank"]],
  [nil, true, true, false, ["Author can't be blank"]],
  [nil, true, true, false, ["Author can't be blank"]],
]
raise "reads: #{got.inspect}" unless got == want
puts "nil belongs_to reads passed"
"#
    );
    let run = app("class Post < ApplicationRecord\n  belongs_to :author, optional: true\n  validates :author_id, presence: true\nend\n").run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("nil belongs_to reads passed"), "{}", run.stdout);
}

#[test]
fn a_nil_belongs_to_saves_null_like_rails() {
    let script = format!(
        "{SETUP}{}",
        r#"[fresh, assigned, cleared].each(&:save!)
got = [Post.where(author_id: nil).count, Post.all.map(&:author_id)]
raise "stored: #{got.inspect}" unless got == [3, [nil, nil, nil]]
c = Comment.new
c.body = "c"
c.save!
pin = Pin.new
pin.subject = c
pin.subject = nil
got = [pin.subject_id, pin.subject_type, pin.subject]
raise "polymorphic: #{got.inspect}" unless got == [nil, nil, nil]
pin.save!
got = [Pin.where(subject_id: nil).count, Pin.where(subject_type: nil).count]
raise "polymorphic stored: #{got.inspect}" unless got == [1, 1]
puts "nil belongs_to storage passed"
"#
    );
    let run = app("class Post < ApplicationRecord\n  belongs_to :author, optional: true\nend\n").run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("nil belongs_to storage passed"), "{}", run.stdout);
}

/// A strict target's key field is the column's non-nil type, so its
/// writer keeps the sentinel (`0`) and its reader treats it as nil. (No
/// polymorphic models here: their type `case` is a separate rust gap.)
#[test]
fn strict_targets_keep_the_sentinel() {
    use roundhouse::project::BuildTarget;
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/author.rb", "class Author < ApplicationRecord\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"authors\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n    t.integer \"author_id\"\n  end\nend\n",
        )
        .write("app/models/post.rb", "class Post < ApplicationRecord\n  belongs_to :author, optional: true\nend\n")
        .emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:#?}");
    let post = std::fs::read_to_string(emitted.join("src/models/post.rs")).expect("post.rs");
    let writer = &post[post.find("pub fn set_author(").expect("set_author")..];
    let writer = &writer[..writer.find("\n    }\n").unwrap_or(writer.len())];
    assert!(writer.contains("= 0"), "{writer}");
}
