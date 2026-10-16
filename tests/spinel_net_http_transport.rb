# Driver for tests/spinel_net_http_start.rs's transport check - COMPILED
# BY SPINEL, like tests/spinel_net_http_start.rb beside it. No stub answers
# these requests, so they go through the reopen's `transport_request` to a
# one-shot HTTP server the harness runs on 127.0.0.1 (its port is ARGV[0]).
#
# THE ORACLE IS CRUBY'S `Net::HTTP#request`: a GET sends
# `Accept-Encoding: gzip;q=1.0,deflate;q=0.6,identity;q=0.3` and, when the
# response comes back gzip-encoded, answers the decoded body with the
# Content-Encoding header removed. A plain response is answered as is.
require_relative "net_http"

def check(name, ok)
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

port = ARGV[0].to_i

plain = Net::HTTP.new("127.0.0.1", port).request(Net::HTTP::Get.new("/plain"))
check "a request no stub answers reaches the server", plain.code == "200"
check "the plain body is answered as sent", plain.body == "plain widget"

gzipped = Net::HTTP.new("127.0.0.1", port).request(Net::HTTP::Get.new("/gzip"))
check "a gzip-encoded response is answered decoded", gzipped.body == "decoded widget"
check "the decoded response drops Content-Encoding", gzipped["content-encoding"].nil?

puts "done"
