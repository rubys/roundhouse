//! `render partial:, collection:, cached: ->(item) { key }` caches each
//! element's fragment under the key the lambda answers, as Rails'
//! `ActionView::CollectionCaching` does, and preloads a collection that
//! can preload before it builds the keys.
//!
//! campfire's room, messages and search pages render this way, over a
//! page loaded with `skip_preloading!`: Rails preloads the page's
//! associations in the collection render. The lowering used to drop a
//! callable `cached:` and walk the collection uncached, so every message
//! loaded its creator, attachment, rich text, room and boosts one at a
//! time — 216 queries for one room page instead of 23.

use super::emit_and_run;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  def self.render_count
    @render_count || 0
  end
  def self.reset_render_count
    @render_count = 0
  end
  def self.bump_render
    @render_count = render_count + 1
  end
  def bump_render
    Article.bump_render
    title
  end
"#,
        )
        .write(
            "app/models/probe_page.rb",
            r#"class ProbePage < Array
  def self.preloads
    @preloads || 0
  end

  def self.reset_preloads
    @preloads = 0
  end

  def self.record_preload
    @preloads = preloads + 1
  end

  def preload_associations(records)
    ProbePage.record_preload
    records
  end
end
"#,
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n\n  def probe\n    @articles = ProbePage.new(Article.order(:title).to_a)\n  end\n",
        )
        .write(
            "app/views/articles/_probe_row.html.erb",
            "<% probe_row.bump_render %><i><%= probe_row.title %></i>\n",
        )
        .write(
            "app/views/articles/probe.html.erb",
            "<%= render partial: \"articles/probe_row\", collection: @articles, cached: ->(article) { [ article.title, \"v1\" ] } %>\n",
        )
}

#[test]
fn a_callable_cache_key_caches_each_element_and_preloads_the_page() {
    let run = overlay().run_ruby(
        r#"
Article.delete_all
3.times { |i| Article.create!(title: "row-#{i}", body: "long enough body") }
page = -> { ProbePage.new(ActiveRecord::Relation.new(Article).to_a.sort_by { |a| a.title }) }
ActionController::Current.controller = ActionController::Base.new

ActionController::Base.perform_caching = true
Article.reset_render_count
ProbePage.reset_preloads
a = Views::Articles.probe(page.call)
raise "first render ran #{Article.render_count}" unless Article.render_count == 3
raise "first render preloaded #{ProbePage.preloads}" unless ProbePage.preloads == 1

Article.reset_render_count
b = Views::Articles.probe(page.call)
raise "warm render ran #{Article.render_count}" unless Article.render_count == 0
raise "html drifted #{a.inspect} vs #{b.inspect}" unless a == b

# One element's key moves: only it renders again, in its place.
row = ActiveRecord::Relation.new(Article).to_a.find { |r| r.title == "row-1" }
row.update!(title: "row-1b")
Article.reset_render_count
c = Views::Articles.probe(page.call)
raise "one changed key ran #{Article.render_count}" unless Article.render_count == 1
raise "order lost: #{c.inspect}" unless c.index("row-0") < c.index("row-1b") && c.index("row-1b") < c.index("row-2")

# Caching off: every element renders, and the page still preloads.
ActionController::Base.perform_caching = false
Article.reset_render_count
ProbePage.reset_preloads
d = Views::Articles.probe(page.call)
raise "uncached ran #{Article.render_count}" unless Article.render_count == 3
raise "uncached preloaded #{ProbePage.preloads}" unless ProbePage.preloads == 1
raise "uncached html #{c.inspect} vs #{d.inspect}" unless c == d
puts "ok"
"#,
    );
    run.assert_passes();
    assert_eq!(run.stdout.trim(), "ok", "stderr:\n{}", run.stderr);
}
