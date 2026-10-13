//! The pagy gem (43.6.2) on real-blog: `@pagy, @articles = pagy(scope,
//! limit: 2)` over a seeded table, the nav and info markup in the view,
//! and the app's 404-past-the-end concern, run through the dispatcher.
//!
//! Every expected string is what the REAL gem produced for the same call:
//! `tests/pagy_oracle/oracle.rb` drives pagy 43.6.2 on ActiveRecord +
//! sqlite and Rack 3.2.7 (the versions EngineeredAt's Gemfile.lock pins)
//! and prints `tests/pagy_oracle/cases.jsonl`, one object per call: the
//! count, request path and query, `limit:`, and what came back (the
//! readers, `series_nav`, `info_tag`, the records' ids).

use super::emit_and_run;

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pagy_oracle/cases.jsonl");

/// EngineeredAt's `Pagination` concern, as written.
const PAGINATION: &str = "module Pagination
  extend ActiveSupport::Concern

  module BoundsCheck
    def pagy(collection, **vars)
      pagy, records = super
      raise ActiveRecord::RecordNotFound if pagy.page > pagy.last

      [pagy, records]
    end
  end

  included do
    include Pagy::Method
    prepend BoundsCheck

    rescue_from Pagy::OptionError do
      raise ActiveRecord::RecordNotFound
    end
  end
end
";

/// real-blog with pagy locked, `ArticlesController#index` paginating
/// `Article.order(:id)` (`pagy_call` is the call, e.g. `pagy(scope,
/// limit: 2)`), the nav and info in the index view, and optionally the
/// app's `Pagination` concern.
fn app(pagy_call: &str, with_concern: bool) -> emit_and_run::Overlay {
    let overlay = emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    pagy (43.6.2)\n      json\n      yaml\n    debug (1.11.1)\n",
        )
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  pagy (~> 43.0)\n")
        .edit(
            "app/controllers/articles_controller.rb",
            "@articles = Article.includes(:comments).order(created_at: :desc)",
            &format!("scope = Article.order(:id)\n    @pagy, @articles = {pagy_call}"),
        )
        .edit(
            "app/views/articles/index.html.erb",
            "    <% end %>\n  </div>\n</div>",
            "    <% end %>\n  </div>\n  <% if @pagy.last > 1 %>\n    <div id=\"nav\"><%== @pagy.series_nav %></div>\n  <% end %>\n  <div id=\"info\"><%== @pagy.info_tag %></div>\n</div>",
        );
    if with_concern {
        overlay
            .write("app/controllers/concerns/pagination.rb", PAGINATION)
            .edit(
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\n",
                "class ApplicationController < ActionController::Base\n  include Pagination\n",
            )
    } else {
        overlay
    }
}

/// Ruby helpers shared by the scripts: the oracle's cases, a GET through
/// the dispatcher, and the part of the body between a div's tags.
const HELPERS: &str = r##"
require "json"
CASES = File.readlines("__CASES__").map { |l| JSON.parse(l) }
def get(path, query)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  [status, body.join]
end
def div(body, id)
  m = body.match(%r{<div id="#{id}">(.*?)</div>}m)
  m && m[1]
end
def titles(body)
  body.scan(/>(Post \d+)</).flatten
end
def seed(n)
  Article.delete_all
  n.times { |i| Article.create!(title: "Post #{i + 1}", body: "body number #{i + 1} is long enough") }
end
def case_for(count, query, limit)
  CASES.find { |c| c["count"] == count && c["path"] == "/articles" && c["query"] == query && c["opts"] == (limit ? { "limit" => limit } : {}) } or raise "no oracle case for #{count} #{query.inspect} #{limit.inspect}"
end
"##;

fn script(body: &str) -> String {
    format!("{}\n{}", HELPERS.replace("__CASES__", CASES), body)
}

/// 7 rows, `limit: 2`: pages 1 to 4 are the gem's markup byte for byte and
/// hold the right rows; a page past the end is a 404 (the app's
/// `BoundsCheck`, prepended on the gem's `pagy` through `super`); a
/// `page` that is not a positive integer reads as 1, as the gem reads it.
#[test]
fn pages_nav_and_the_404_past_the_end() {
    app("pagy(scope, limit: 2)", true)
        .run_ruby(&script(
            r##"
seed(7)
rows = {
  nil => ["Post 1", "Post 2"], "1" => ["Post 1", "Post 2"], "2" => ["Post 3", "Post 4"],
  "3" => ["Post 5", "Post 6"], "4" => ["Post 7"],
}
rows.each do |page, want|
  query = page ? "page=#{page}" : ""
  status, body = get("/articles", query)
  raise "GET /articles?#{query} answered #{status}" unless status == 200
  raise "page #{page.inspect}: rows #{titles(body).inspect}, want #{want.inspect}" unless titles(body) == want
  oracle = case_for(7, query, 2)
  raise "page #{page.inspect}: nav\n#{div(body, "nav")}\nwant\n#{oracle["series_nav"]}" unless div(body, "nav") == oracle["series_nav"]
  raise "page #{page.inspect}: info\n#{div(body, "info")}\nwant\n#{oracle["info_tag"]}" unless div(body, "info") == oracle["info_tag"]
end

# other query pairs ride along, in order, with `page` replaced in place
status, body = get("/articles", "sort=top&page=2")
oracle = case_for(7, "sort=top&page=2", 2)
raise "sort=top: #{div(body, "nav")}" unless status == 200 && div(body, "nav") == oracle["series_nav"]
status, body = get("/articles", "page=3&sort=top")
oracle = case_for(7, "page=3&sort=top", 2)
raise "page=3&sort=top: #{div(body, "nav")}" unless status == 200 && div(body, "nav") == oracle["series_nav"]

# past the end: the gem alone answers an empty page (see the next test);
# the app's concern turns it into RecordNotFound, a 404
%w[page=5 page=9999].each do |query|
  status, = get("/articles", query)
  raise "GET /articles?#{query} answered #{status}, want 404" unless status == 404
end

# not a positive integer: page 1, as `String#to_i` and the floor read it
["page=0", "page=-3", "page=abc", "page="].each do |query|
  status, body = get("/articles", query)
  raise "GET /articles?#{query} answered #{status}" unless status == 200 && titles(body) == ["Post 1", "Post 2"]
end
status, body = get("/articles", "page=3abc")
raise "page=3abc answered #{status}" unless status == 200 && titles(body) == ["Post 5", "Post 6"]
puts "PASS"
"##,
        ))
        .assert_passes();
}

/// Without the app's concern, a page past the end is what the gem
/// answers: an EMPTY page with `previous` pointing at the last page, and
/// the nav and info the gem renders for it.
#[test]
fn the_gem_alone_serves_an_empty_page_past_the_end() {
    app("pagy(scope, limit: 2)", false)
        .run_ruby(&script(
            r##"
seed(7)
status, body = get("/articles", "page=5")
raise "answered #{status}" unless status == 200
raise "rows #{titles(body).inspect}" unless titles(body) == []
oracle = case_for(7, "page=5", 2)
raise "nav\n#{div(body, "nav")}\nwant\n#{oracle["series_nav"]}" unless div(body, "nav") == oracle["series_nav"]
raise "info #{div(body, "info")}" unless div(body, "info") == oracle["info_tag"]
puts "PASS"
"##,
        ))
        .assert_passes();
}

/// No `limit:` is the gem's default of 20: 45 rows are three pages, the
/// last holding 5.
#[test]
fn the_default_limit_is_twenty() {
    app("pagy(scope)", true)
        .run_ruby(&script(
            r##"
seed(45)
{ "" => 20, "page=2" => 20, "page=3" => 5 }.each do |query, n|
  status, body = get("/articles", query)
  raise "GET /articles?#{query} answered #{status}" unless status == 200
  raise "#{query}: #{titles(body).length} rows, want #{n}" unless titles(body).length == n
  oracle = case_for(45, query, nil)
  raise "#{query}: nav #{div(body, "nav")}" unless div(body, "nav") == oracle["series_nav"]
  raise "#{query}: info #{div(body, "info")}" unless div(body, "info") == oracle["info_tag"]
end
status, = get("/articles", "page=4")
raise "page=4 answered #{status}, want 404" unless status == 404
puts "PASS"
"##,
        ))
        .assert_passes();
}

/// Every call the oracle recorded, against `Pagy` directly: the paginator
/// state, `series_nav` and `info_tag` equal the gem's, including the
/// deep pages with gaps, a page past the end, and the query strings the
/// links have to carry (repeated keys, `+`, escapes, non-ASCII). A key
/// with `[`/`]` (a nested parameter) is refused by name, and a malformed
/// `%` escape raises where Rack answers 400.
#[test]
fn the_oracle_matrix() {
    app("pagy(scope, limit: 2)", true)
        .run_ruby(&script(
            r##"
checked = 0
CASES.each do |c|
  opts = c["opts"]
  next unless opts.keys - ["limit"] == []   # `page:` etc. are refused at analysis
  limit = opts["limit"] || 20
  query = c["query"]
  if c["error"]
    begin
      Pagy.new(c["count"], Pagy.page_param(query), limit, c["path"], query)
      raise "#{query.inspect}: the gem raised #{c["error"]}, this did not"
    rescue ArgumentError
    end
    next
  end
  if query =~ /[\[\]]|%5[bBdD]/
    begin
      Pagy.new(c["count"], Pagy.page_param(query), limit, c["path"], query)
      raise "#{query.inspect}: a nested parameter was linked"
    rescue NotImplementedError
    end
    next
  end
  pagy = Pagy.new(c["count"], Pagy.page_param(query), limit, c["path"], query)
  got = { "page" => pagy.page, "limit" => pagy.limit, "last" => pagy.last, "total" => pagy.count,
          "offset" => pagy.offset, "from" => pagy.from, "to" => pagy.to,
          "previous" => pagy.previous, "next" => pagy.next,
          "series_nav" => pagy.series_nav, "info_tag" => pagy.info_tag }
  got.each do |key, value|
    raise "#{c["count"]} #{c["path"]}?#{query} limit #{limit}: #{key}\n got #{value.inspect}\nwant #{c[key].inspect}" unless value == c[key]
  end
  checked += 1
end
raise "only #{checked} cases checked" unless checked > 60
puts "PASS #{checked}"
"##,
        ))
        .assert_passes();
}

/// `items: 5` is not an option pagy 43 reads: the page size is the
/// default 20, whatever the number (checked against the gem, which
/// answers `limit` 20 and 20 rows for `items: 50`).
#[test]
fn items_is_ignored_as_the_gem_ignores_it() {
    app("pagy(scope, items: 5)", false)
        .run_ruby(&script(
            r##"
seed(25)
status, body = get("/articles", "")
raise "answered #{status}" unless status == 200
raise "#{titles(body).length} rows, want 20" unless titles(body).length == 20
status, body = get("/articles", "page=2")
raise "page 2 has #{titles(body).length} rows, want 5" unless titles(body).length == 5
puts "PASS"
"##,
        ))
        .assert_passes();
}
