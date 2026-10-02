# Fetches every corpus case from Rails and from the emit — GET
# /_richtext/:id, the overlay page (overlay.rb) — and compares each field.
#
#   ruby tests/richtext_corpus/compare.rb RAILS_PORT EMIT_PORT expected.json corpus.sqlite3 [-v]
#
# Rails-served is the oracle for the emit. It is itself checked against
# expected.json (what generate.rb recorded in-process), so a harness
# fault — the overlay rendering differently from `message_presentation`
# called directly — shows up as its own count rather than as emit
# failures. Prints a table per field and case group, and the first few
# failing cases.
require "json"
require "net/http"
require "open3"
require "cgi"
require "nokogiri"

# once-campfire-rust's `normalized_dom` (crates/richtext/tests/corpus.rs),
# ported: parse as an HTML5 fragment, drop whitespace-only text, collapse
# whitespace runs, sort attributes. Two renders equal under it say the
# same thing to a browser; the byte column says whether they are also
# the same bytes.
def dom(html)
  out = +""
  walk = lambda do |node|
    node.children.each do |c|
      if c.text?
        t = c.text.split.join(" ")
        out << t unless t.empty?
      elsif c.element?
        attrs = c.attribute_nodes.map { |a| [a.name, a.value] }.sort
        out << "<#{c.name} #{attrs.inspect}>"
        walk.call(c)
        out << "</#{c.name}>"
      end
    end
  end
  # The corpus nests past Gumbo's default depth of 400 on purpose.
  walk.call(Nokogiri::HTML5.fragment(html.to_s, max_tree_depth: -1))
  out
rescue ArgumentError
  "unparseable: #{html}"
end

rails_port, emit_port, expected_path, db = ARGV
verbose = ARGV.include?("-v")
# FAIL lines for byte differences too, not only for DOM differences.
$verbose_bytes = ARGV.include?("--bytes")
cases = JSON.parse(File.read(expected_path)).fetch("cases")

out, status = Open3.capture2("sqlite3", db, "SELECT client_message_id, id FROM messages WHERE client_message_id LIKE 'corpus-%'")
abort "sqlite3 failed" unless status.success?
ids = out.lines.to_h { |l| k, v = l.chomp.split("|"); [k, v] }

UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36"
FIELDS = %w[presentation plain_text].freeze

def fetch(http, id, host)
  req = Net::HTTP::Get.new("/_richtext/#{id}")
  req["Host"] = host
  req["User-Agent"] = UA
  res = http.request(req)
  return { "status" => res.code } unless res.code == "200"
  body = res.body.to_s.force_encoding("UTF-8")
  FIELDS.to_h { |f| [f, body[/<!--RC:#{f}-->(.*?)<!--\/RC:#{f}-->/m, 1]] }.merge("status" => "200")
end

def group(name) = name.start_with?("fuzz ") ? "fuzz" : name.start_with?("mutation ") ? "mutation" : "handwritten"

rails_http = Net::HTTP.start("127.0.0.1", rails_port.to_i, read_timeout: 30)
emit_http = Net::HTTP.start("127.0.0.1", emit_port.to_i, read_timeout: 30)

tally = Hash.new { |h, k| h[k] = Hash.new(0) }
failures = Hash.new { |h, k| h[k] = [] }
harness = Hash.new(0)
cases.each_with_index do |c, i|
  id = ids["corpus-#{i}"] or abort "no message for case #{i} (#{c["name"]})"
  host = c["host"] || "once.campfire.test"
  rails = fetch(rails_http, id, host)
  emit = begin
    fetch(emit_http, id, host)
  rescue => e
    emit_http = Net::HTTP.start("127.0.0.1", emit_port.to_i, read_timeout: 30) rescue nil
    { "status" => "#{e.class}" }
  end
  g = group(c["name"])
  # The oracle against its own in-process recording.
  if rails["status"] == "200"
    want = c["presentation"].is_a?(Hash) ? c["presentation"]["ok"] : nil
    harness[want.nil? || rails["presentation"] == want ? :presentation_ok : :presentation_off] += 1
    plain = c["plain_text"].is_a?(Hash) && c["plain_text"]["ok"] ? CGI.escapeHTML(c["plain_text"]["ok"]) : nil
    harness[plain.nil? || rails["plain_text"] == plain ? :plain_ok : :plain_off] += 1
  else
    harness[:rails_http_error] += 1
  end
  FIELDS.each do |f|
    key = [f, g]
    tally[key][:total] += 1
    if rails["status"] != "200"
      # Rails raised on this body (a 500): the emit matches by failing
      # too, whatever its page would have said.
      if emit["status"] != "200"
        tally[key][:match] += 1
        tally[key][:dom] += 1
      else
        failures[key] << [c["name"], "Rails answered #{rails["status"]}", "emit answered 200"]
      end
    elsif emit["status"] != "200"
      tally[key][:emit_error] += 1
      failures[key] << [c["name"], "emit answered #{emit["status"]}", ""] if f == "presentation"
    elsif rails[f] == emit[f]
      tally[key][:match] += 1
      tally[key][:dom] += 1
    else
      same_dom = f == "presentation" && dom(rails[f]) == dom(emit[f])
      tally[key][:dom] += 1 if same_dom
      failures[key] << [c["name"], rails[f].to_s, emit[f].to_s] unless same_dom && !$verbose_bytes
    end
  end
end

puts "oracle check (Rails-served vs expected.json): presentation #{harness[:presentation_ok]}/#{harness[:presentation_ok] + harness[:presentation_off]}, " \
     "plain_text #{harness[:plain_ok]}/#{harness[:plain_ok] + harness[:plain_off]}; " \
     "#{harness[:rails_http_error]} bodies Rails itself fails on (the emit must fail on them too)"
puts format("%-13s %-12s %9s %9s %7s", "field", "cases", "bytes", "dom", "error")
FIELDS.each do |f|
  %w[handwritten fuzz mutation].each do |g|
    t = tally[[f, g]]
    next if t[:total].zero?
    dom_col = f == "presentation" ? format("%4d/%-4d", t[:dom], t[:total]) : "        -"
    puts format("%-13s %-12s %4d/%-4d %s %7d", f, g, t[:match], t[:total], dom_col, t[:emit_error])
  end
end
failures.each do |(f, g), list|
  list.first(verbose ? 1000 : 3).each do |name, want, got|
    # Around the first character the two differ at, not the heads: two
    # renders usually agree for a long stretch first.
    at = 0
    at += 1 while at < want.length && at < got.length && want[at] == got[at]
    from = [at - 60, 0].max
    puts "FAIL #{f} [#{g}] #{name} (first difference at #{at})"
    puts "     rails …#{want[from, 200].to_s.gsub("\n", "\\n")}"
    puts "     emit  …#{got[from, 200].to_s.gsub("\n", "\\n")}"
  end
end
puts "done"
