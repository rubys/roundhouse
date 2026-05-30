require_relative "test_helper"
# Fixture modules the index renders comments from. Emitted tests get
# these injected by src/emit/ruby.rs; this hand-written test names them
# explicitly. SchemaSetup.reset! (run from `setup`) dispatches their
# `_fixtures_load!` to seed the articles + comments.
require_relative "fixtures/articles"
require_relative "fixtures/comments"

# Query-count harness (issue #27).
#
# `compare` checks byte-identical HTML and is structurally blind to the
# `includes(:assoc)` N+1: eager-load and N+1 render the *same bytes* —
# only the query strategy differs, so every checkmark stays green
# either way. A query counter is the only instrument that can see it.
# This mirrors how Rails tests the same property: `assert_queries_count`
# (activerecord testing/query_assertions.rb) subscribes a `SQLCounter`
# to the `sql.active_record` notification around a block and asserts on
# the count. Our analog is `Db.capture_sql { … }`, which records the SQL
# every prepare/exec issues through the single Db funnel.
#
# `/articles` does `Article.includes(:comments).order(created_at: :desc)`
# then the index view's `pluralize(article.comments.size, "comment")`.
# Rails issues exactly **2** queries — the articles SELECT plus the one
# batched comments preload — and `.comments.size` then reads the loaded
# association in memory (0 queries). The preload lowering (issue #27)
# makes roundhouse match that. Without it the request is **1 + N**: one
# articles query plus a fresh `WHERE article_id = <id>` comments SELECT
# per article. This test pins the count at 2 and fails the moment the
# N+1 returns.
class QueryCountTest < ActionDispatch::IntegrationTest
  def test_articles_index_is_two_queries_not_n_plus_one
    sql = Db.capture_sql { get "/articles" }
    assert_response :success

    # Bound: parent SELECT + one batched comments preload. NOT
    # 1 + (one comments query per article). The message dumps the log
    # on failure, the way Rails' assert_queries_count does.
    count = sql.length
    unless count == 2
      raise "expected /articles to issue 2 queries (articles + batched " \
            "comments preload), got #{count}:\n#{sql.join("\n")}"
    end

    # Shape assertion, mirroring Rails'
    # `assert_no_queries_match(/WHERE article_id = N/)`: the eager path
    # batches with `IN (...)`; the lazy accessor emits a single-id
    # equality filter. Any per-article equality filter on comments
    # means the cache was missed and the lazy `where` fired — the
    # regression — even if the bare count assertion above were ever
    # loosened.
    per_article = sql.select { |q| q =~ /FROM comments WHERE article_id = \d/ }
    unless per_article.empty?
      raise "found per-article comment queries (N+1 regression):\n" \
            "#{per_article.join("\n")}"
    end
  end
end

# Self-driving footer — the hand-written counterpart to the emitted
# tests' autorun shim (src/emit/ruby.rs::render_autorun_shim). This file
# rides into the emitted project verbatim via runtime/spinel/test/, so
# it carries its own driver rather than leaning on Minitest's at-exit
# autorun: that keeps it independent of the Minitest-vs-TestBase split
# and spinel-AOT safe. `setup` runs SchemaSetup.reset!, seeding the
# article + comment fixtures the index renders.
__t = QueryCountTest.new
__t.setup
__t.test_articles_index_is_two_queries_not_n_plus_one
__t.teardown
puts "QueryCountTest: 1 tests passed"
