//! `clear_<col>_change` and `restore_<col>!` answer ActiveModel::Dirty's: the first forgets a column's pending change (the new value stays), the second writes the original back and forgets it.

use super::emit_and_run;

// `title_changed?` / `body_changed?` are synthesized on demand, so the model names them.
const SETTLE: &str = r##"  def pending = [title_changed?, body_changed?]
  def settle
    clear_title_change
    restore_body!
  end
"##;

#[test]
fn clear_change_and_restore_bang_run_as_active_model_dirty() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{SETTLE}"))
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
a = Article.create!(title: "first", body: "body number one is long enough")
a.title = "second"
a.body = "body number two is long enough"
check("pending", a.pending, [true, true])
a.settle
check("clear keeps the value", [a.title, a.pending[0]], ["second", false])
check("restore writes the original back", [a.body, a.pending[1]], ["body number one is long enough", false])
check("nothing pending", a.changed?, false)
a.restore_body!
check("restore with nothing pending", a.body, "body number one is long enough")
puts "ok"
"##,
        )
        .assert_passes();
}
