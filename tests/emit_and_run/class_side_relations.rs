//! `Article.find_each { … }`, a bare `pluck(:title)` in a class method,
//! `Article.order(:title).first`: Rails' `Querying` hands each class-side
//! query method to `all`. The lowerings re-root only the shapes they
//! recognize, so the rest reached a class with no such method, and the
//! emitted program raised NoMethodError where `check` was clean
//! (lobsters' `Link.find_each`, mastodon's `MediaAttachment.sum`).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def self.titles\n    pluck(:title)\n  end\n\n  def self.shouted\n    out = []\n    find_each { |a| out << a.title.upcase }\n    out\n  end\n\n  def self.last_by_title\n    order(:title).last\n  end\n\n  def self.title_count\n    distinct.ids.length\n  end\n",
    )
}

const ASSERTIONS: &str = r##"Article.create!(title: "b", body: "a body long enough")
Article.create!(title: "a", body: "a body long enough")
raise Article.titles.inspect unless Article.titles.sort == ["a", "b"]
raise Article.shouted.inspect unless Article.shouted.sort == ["A", "B"]
raise Article.last_by_title.title unless Article.last_by_title.title == "b"
raise Article.title_count.inspect unless Article.title_count == 2
puts "class-side relations passed"
"##;

#[test]
fn class_side_query_methods_run() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    app().run_ruby(ASSERTIONS).assert_passes();
}

