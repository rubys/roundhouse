# Oracle: pagy 43.6.2 (the version EngineeredAt's Gemfile.lock pins), driven
# exactly as the app drives it: Pagy::Method#pagy(scope[, limit: n]) in a
# controller (request = path + query string), then `@pagy.series_nav` /
# `@pagy.info_tag` and the readers the views call. Prints one JSON object per
# case. The collection is an ActiveRecord relation over sqlite (the app's
# shape), so `records` is the real windowed relation.
#
# run: ruby -I gems/gems/pagy-43.6.2/lib oracle.rb > cases.jsonl
require "json"
require "active_record"
require "rack"
require "pagy"

ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define { create_table(:items) { |t| t.string :name } }
class Item < ActiveRecord::Base; end

class FakeController
  include Pagy::Method
  def initialize(path, query) = (@path = path; @query = query)
  # Pagy::Request reads base_url, path, GET, POST, cookies off the request
  def request
    q = @query
    path = @path
    Struct.new(:base_url, :path, :GET, :POST, :cookies).new("http://example.org", path, Rack::Utils.parse_nested_query(q), {}, {})
  end
  public :pagy
end

def run(count, path, query, **opts)
  begin
    run_one(count, path, query, **opts)
  rescue StandardError => e
    puts JSON.generate({ "count" => count, "path" => path, "query" => query, "opts" => opts.transform_keys(&:to_s), "error" => e.class.name })
  end
end

def run_one(count, path, query, **opts)
  Item.delete_all
  Item.insert_all((1..count).map { |i| { id: i, name: "n#{i}" } }) if count > 0
  c = FakeController.new(path, query)
  pagy, records = c.pagy(Item.order(:id), **opts)
  out = {
    "count" => count, "path" => path, "query" => query, "opts" => opts.transform_keys(&:to_s),
    "page" => pagy.page, "limit" => pagy.limit, "last" => pagy.last,
    "total" => pagy.count, "offset" => pagy.offset, "from" => pagy.from, "to" => pagy.to,
    "previous" => pagy.previous, "next" => pagy.next,
    "ids" => records.pluck(:id),
    "series_nav" => pagy.series_nav,
    "info_tag" => pagy.info_tag,
  }
  puts JSON.generate(out)
end

[0, 1, 20, 21, 45, 131].each do |n|
  [nil, "2", "3", "7"].each do |pg|
    q = pg ? "page=#{pg}" : ""
    run(n, "/articles", q)
  end
end
# deep pages: gaps on the left, right, both
run(400, "/articles", "page=10", limit: 5)
run(400, "/articles", "page=40", limit: 5)
run(400, "/articles", "page=75", limit: 5)
run(400, "/articles", "page=76", limit: 5)
run(400, "/articles", "page=77", limit: 5)
run(400, "/articles", "page=80", limit: 5)
run(400, "/articles", "page=81", limit: 5)   # past the end
run(400, "/articles", "page=9999", limit: 5)
run(45, "/articles", "page=4")               # past the end, last=3
run(45, "/articles", "page=0")
run(45, "/articles", "page=-3")
run(45, "/articles", "page=abc")
run(45, "/articles", "page=2abc")
run(45, "/articles", "page=")
run(45, "/articles", "page=2&page=3")
# existing query keys are kept, in order; page replaced in place or appended
run(100, "/search", "q=ruby+gems&sort=top&page=2", limit: 10)
run(100, "/search", "page=2&q=ruby", limit: 10)
run(100, "/search", "q=a%26b%3Dc&topic=x%2Fy", limit: 10)
run(100, "/search", "q=h%C3%A9llo+w%C3%B6rld&page=3", limit: 10)
run(100, "/topics/ruby-on-rails", "sort=new", limit: 10)
run(100, "/profiles/alice/comments", "", limit: 25)
run(100, "/a&b", "x=1", limit: 10)
run(100, "/articles", "flag&page=2", limit: 10)
run(100, "/articles", "empty=&page=2", limit: 10)
run(100, "/articles", "page=2&limit=3", limit: 10)  # limit key is not honored without max_limit; and is removed
run(100, "/articles", "x=%3Cb%3E&page=2", limit: 10)
run(100, "/articles", "x=a%22b&page=2", limit: 10)
# options
run(100, "/articles", "page=3", limit: 7)
run(100, "/articles", "page=3", limit: 100)
run(100, "/articles", "page=1", limit: 200)
run(100, "/articles", "page=2", page: 4, limit: 10)  # explicit page: beats the param

# the controller flow of the emit-and-run test: 7 rows, limit 2
[nil, 1, 2, 3, 4, 5].each { |pg| run(7, "/articles", pg ? "page=#{pg}" : "", limit: 2) }
run(7, "/articles", "sort=top&page=2", limit: 2)
run(7, "/articles", "page=3&sort=top", limit: 2)
# unusual query strings
run(100, "/articles", "a=1&&b=2&page=2", limit: 10)
run(100, "/articles", "=x&page=2", limit: 10)
run(100, "/articles", "a=1&a=2&page=2", limit: 10)
run(100, "/articles", "a=1;b=2&page=2", limit: 10)
run(100, "/articles", "a=1&+b=2&page=2", limit: 10)
run(100, "/articles", "a+b=c+d&page=2", limit: 10)
run(100, "/articles", "\u00e9=\u00fc&page=2", limit: 10)
run(100, "/articles", "a=%2B+%20%7E%2A&page=2", limit: 10)
run(100, "/articles", "%70age=2", limit: 10)
run(100, "/articles", "page=%32", limit: 10)
run(100, "/articles", "page=+2", limit: 10)
run(100, "/articles", "page=2%20", limit: 10)
run(100, "/articles", "page=0x3", limit: 10)
run(100, "/articles", "page=1_0", limit: 10)
run(100, "/articles", "page=3&limit=3&x=1", limit: 10)
run(100, "/articles", "a=%zz&page=2", limit: 10)
run(100, "/articles", "a=%&page=2", limit: 10)
run(100, "/articles", "k%5Bx%5D=1&page=2", limit: 10)
run(100, "/articles", "k[x]=1&page=2", limit: 10)
run(100, "/articles", "k[]=1&page=2", limit: 10)
run(100, "/articles", "page[]=2", limit: 10)
