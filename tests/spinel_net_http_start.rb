# Driver for tests/spinel_net_http_start.rs — COMPILED BY SPINEL, not run
# under CRuby: runtime/spinel/net_http.rb reopens spinel's net package
# (`build_response`, `open_connection`, `@ipaddr`), which CRuby's
# Net::HTTP does not spell, and the ruby family never loads it. The
# harness copies it, http_stub.rb and tcp_socket_stub.rb beside this file.
#
# THE ORACLE IS CRUBY'S `Net::HTTP#start`: with a block it yields the
# session, answers the block's value and closes the session in an ensure;
# without one it answers the session. Since matz/spinel#8048 the package's
# `Net::HTTP.start`, `.get_response` and `.post_form` all go through that
# block form, so the reopened lazy `#start` has to keep it.
#
# Every request below is answered by the stub table, so nothing here
# needs the network. A dropped block sends no request at all, which is
# why the checks look at what the block did as well as what came back.
require_relative "net_http"

def check(name, ok)
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

HttpStub.stub("GET", "http://widgets.example/gadget", 200, "gadget body", { "content-type" => "text/plain" })
HttpStub.stub("POST", "http://widgets.example/gadgets", 201, "created", { "content-type" => "text/plain" })

# The block's value is the answer, and the block ran.
ran = 0
value = Net::HTTP.start("widgets.example", 80) { |http| ran += 1; 42 }
check "Net::HTTP.start answers the block's value", value == 42
check "Net::HTTP.start runs the block once", ran == 1

# The shape campfire's Opengraph::Fetch and most callers write.
res = Net::HTTP.start("widgets.example", 80) { |http| http.request(Net::HTTP::Get.new("/gadget")) }
check "Net::HTTP.start answers the response the block made", res.is_a?(Net::HTTPOK)
check "the response carries the stubbed body", res.body == "gadget body"

# The session lasts for the block and no longer.
seen = Net::HTTP.new("widgets.example", 80)
inside = false
Net::HTTP.start("widgets.example", 80) { |http| seen = http; inside = http.started? }
check "the session is started inside the block", inside
check "the session is finished after the block", !seen.started?

# A block that finishes the session itself does not make the ensure raise
# (the package's #finish raises IOError on a session that is not open).
closed_early = Net::HTTP.start("widgets.example", 80) { |http| http.finish; "closed" }
check "a block that calls finish itself still answers its value", closed_early == "closed"

# The session is closed when the block raises, and the error propagates.
raised = ""
begin
  Net::HTTP.start("widgets.example", 80) { |http| seen = http; raise ArgumentError, "widget" }
rescue ArgumentError => e
  raised = e.message
end
check "an error raised in the block propagates", raised == "widget"
check "the session is finished after a raising block", !seen.started?

# The class methods #8048 moved onto the block form.
got = Net::HTTP.get_response(URI("http://widgets.example/gadget"))
check "Net::HTTP.get_response answers the response", got.is_a?(Net::HTTPOK) && got.body == "gadget body"
posted = Net::HTTP.post_form(URI("http://widgets.example/gadgets"), { "name" => "widget" })
check "Net::HTTP.post_form answers the response", posted.code == "201" && posted.body == "created"

# Without a block, #start stays lazy: started, the session itself, and
# no socket until a request no stub answers.
http = Net::HTTP.new("widgets.example", 80)
started = http.start
check "a block-less start answers the session", started.equal?(http)
check "a block-less start leaves the session started", http.started?
check "a lazily started session serves a stubbed request", http.request(Net::HTTP::Get.new("/gadget")).body == "gadget body"
http.finish
check "finish closes a lazily started session", !http.started?

# A second #start on an already-open session raises IOError, matching
# CRuby's own guard (`raise IOError, 'HTTP session already opened' if
# @started`, checked ahead of connecting or yielding), and leaves the
# still-open session untouched by the failed restart.
nested = Net::HTTP.new("widgets.example", 80)
nested.start
raised_nested = ""
begin
  nested.start
rescue IOError => e
  raised_nested = e.message
end
check "starting an already-started session raises IOError", raised_nested == "HTTP session already opened"
check "the session is still started after the failed restart", nested.started?
check "the already-started session still serves a stubbed request", nested.request(Net::HTTP::Get.new("/gadget")).body == "gadget body"
nested.finish
check "finish still closes the session normally afterwards", !nested.started?

# Same guard with a block: the block never runs, the outer session is
# not finished out from under its caller, and the error propagates.
nested_blk = Net::HTTP.new("widgets.example", 80)
nested_blk.start
ran_nested_block = false
raised_nested_block = ""
begin
  nested_blk.start { |h| ran_nested_block = true }
rescue IOError => e
  raised_nested_block = e.message
end
check "starting an already-started session with a block raises IOError", raised_nested_block == "HTTP session already opened"
check "the block does not run when the nested start raises", !ran_nested_block
check "the session stays started after the failed block restart", nested_blk.started?
nested_blk.finish
check "finish still closes the session after a failed block restart", !nested_blk.started?

puts "done"
