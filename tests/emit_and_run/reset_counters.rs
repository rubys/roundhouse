//! `Article.reset_counters(id, :comments)` recounts the has_many into its
//! `comments_count` column (lobsters' `Origin.reset_counters(origin.id,
//! :stories)`). The recount is written before analysis too, so the call
//! checks clean where the emit runs it.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "    t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"", "    t.text \"body\"\n    t.integer \"comments_count\", default: 0, null: false\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.recount(id)\n    Article.reset_counters(id, :comments)\n  end\n",
        )
}

const ASSERTIONS: &str = r##"a = Article.create!(title: "t", body: "a body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: a.id, commenter: "y", body: "two")
Article.where(id: a.id).update_all(comments_count: 0)
Article.recount(a.id)
got = Article.find(a.id).comments_count
raise got.inspect unless got == 2
puts "reset counters passed"
"##;

#[test]
fn reset_counters_recounts_the_association() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn reset_counters_checks_clean() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}
