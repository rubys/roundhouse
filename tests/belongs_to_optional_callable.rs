//! `belongs_to :x, optional: <callable>` (activerecord 8.1.4, oracle
//! `optional_proc.rb`). Rails reads `optional:` for its truth, not its
//! result: a lambda (with or without the record), a Symbol, or any other
//! value but `false` / `nil` (parenthesized or not) makes the association optional, so no
//! "must exist" is added, whatever the callable would return.
//!
//! The option was read only as a literal Boolean, so anything else fell
//! back to required and every record failed "Article must exist".

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("optional whatever the lambda answers", [Comment.new(commenter: "draft", body: "b").valid?, Comment.new(commenter: "real", body: "b").valid?], [true, true])
check("a Symbol is optional too", Note.new(commenter: "n", body: "b").valid?, true)
held = Remark.new(commenter: "r", body: "b")
check("optional: false stays required", [held.valid?, held.errors], [false, ["Article must exist"]])
check("optional: (false) is false too", Draft.new(commenter: "d", body: "b").valid?, false)
check("optional: (nil) is nil too", Sketch.new(commenter: "s", body: "b").valid?, false)
puts "optional callable passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, optional: -> { commenter == \"draft\" }\n")
        .write(
            "app/models/note.rb",
            "class Note < ApplicationRecord\n  self.table_name = \"comments\"\n  belongs_to :article, optional: :draft?\n\n  def draft?\n    false\n  end\nend\n",
        )
        .write(
            "app/models/remark.rb",
            "class Remark < ApplicationRecord\n  self.table_name = \"comments\"\n  belongs_to :article, optional: false\nend\n",
        )
        .write(
            "app/models/draft.rb",
            "class Draft < ApplicationRecord\n  self.table_name = \"comments\"\n  belongs_to :article, optional: (false)\nend\n",
        )
        .write(
            "app/models/sketch.rb",
            "class Sketch < ApplicationRecord\n  self.table_name = \"comments\"\n  belongs_to :article, optional: (nil)\nend\n",
        )
}

#[test]
fn a_callable_optional_makes_belongs_to_optional_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("optional callable passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_callable_optional_makes_belongs_to_optional_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("optional callable passed"), "{}", run.stdout);
}
