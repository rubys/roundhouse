//! A model's own accessor for a column is the one Rails calls: a writer
//! runs on `new(…)` and assignment but not when a row loads, a reader on
//! every read, and its `super` is the attribute itself. An override of
//! an association's accessor is not carried yet, and is reported.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def title=(value)\n    self[:title] = value.to_s + \"!\"\n  end\n\n  def body=(text)\n    text = text.to_s.strip\n    super\n  end\n\n  def body\n    super.to_s.upcase\n  end\n",
    )
}

const ASSERTIONS: &str = r##"a = Article.new(title: "hi", body: "  a body long enough  ")
got = [a.title, a.body]
a.save!
b = Article.find(a.id)
got = got + [b.title, b.body]
b.title = "yo"
got << b.title
raise got.inspect unless got == ["hi!", "A BODY LONG ENOUGH", "hi!", "A BODY LONG ENOUGH", "yo!"]
puts "accessor override passed"
"##;

#[test]
fn a_column_accessor_override_is_the_one_called() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn an_association_accessor_override_is_reported() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def comments\n    []\n  end\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("`Article#comments` overrides the accessor its association defines")), "{errors:#?}");
}

/// The override's `super` is the generated writer, `normalizes` included,
/// not a `[]=` the model defines.
#[test]
fn an_override_s_super_runs_the_generated_writer() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n  normalizes :title, with: ->(t) { t.strip }\n\n  def title=(value)\n    super(value.to_s + \"!  \")\n  end\n",
        )
        .run_ruby("a = Article.new(title: \"  hi\", body: \"a body long enough\")\nraise a.title.inspect unless a.title == \"hi!\"\nputs \"normalized\"\n")
        .assert_passes();
}
