//! `save` writes only the columns with pending changes, as Rails'
//! partial updates do. Two copies of one row that each change a different
//! column both keep their change (a worker's jobs on one record, or two
//! requests), a save with nothing pending writes nothing and leaves
//! `updated_at` alone, and `update_columns` writes only the columns it
//! names.

use super::emit_and_run;

// `title_changed?` / `body_changed?` are synthesized on demand, so the model names them.
const PENDING: &str = "  def pending = [title_changed?, body_changed?]\n";

#[test]
fn save_writes_only_changed_columns() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{PENDING}"))
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
a = Article.create!(title: "first", body: "body number one is long enough")

x = Article.find(a.id)
y = Article.find(a.id)
x.title = "title from x"
x.save!
y.body = "body from y is long enough"
y.save!
r = Article.find(a.id)
check("both changes kept", [r.title, r.body], ["title from x", "body from y is long enough"])

r.update_columns(updated_at: "2001-02-03 04:05:06")
stamp = Article.find(a.id).updated_at
q = Article.find(a.id)
q.save!
check("a save with nothing pending leaves updated_at", Article.find(a.id).updated_at, stamp)

z = Article.find(a.id)
z.title = "pending title"
z.update_columns(body: "body from update_columns, long enough")
r = Article.find(a.id)
check("update_columns writes only its columns", [r.title, r.body], ["title from x", "body from update_columns, long enough"])
check("the other change is still pending", z.pending, [true, false])
z.save!
check("and the next save writes it", Article.find(a.id).title, "pending title")
puts "ok"
"##,
        )
        .assert_passes();
}
