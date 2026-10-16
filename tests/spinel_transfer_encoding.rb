# Tep has no request chunk decoder: never dispatch a transfer-coded body.
require_relative "tep_server_harness"

FIELDS = [
  "Transfer-Encoding: chunked\r\n",
  "Transfer-Encoding: gzip\r\n",
  "Transfer-Encoding: identity\r\n",
  "Transfer-Encoding: chunked, gzip\r\n",
  "Transfer-Encoding : chunked\r\n",
  "Transfer-Encoding\t: chunked\r\n",
  "Transfer-Encoding: chunked\r\n gzip\r\n",
  "Transfer-Encoding: chunked\r\nContent-Length: nope\r\n",
  "Transfer-Encoding: chunked\r\nContent-Length: 9999999999999999999999999\r\n",
  "Transfer-Encoding: gzip, chunked\r\n",
  "Transfer-Encoding:\r\n",
  "tRaNsFeR-EnCoDiNg: CHUNKED\r\n",
  "Transfer-Encoding: chunked\r\nContent-Length: 2\r\n",
  "Transfer-Encoding: chunked\r\nTransfer-Encoding:\r\n"
].freeze

SERVERS.each_key do |server|
  ["1.0", "1.1"].each do |version|
    FIELDS.each do |fields|
      bytes = "POST /posts HTTP/#{version}\r\nHost: localhost\r\n#{fields}\r\n" \
              "2\r\né\r\n0\r\n\r\n" + "x" * 8192
      result = serve(server, bytes)
      status, recvs, bodies, raised, keep_alive = result
      check(
        "#{server}: #{fields.inspect} is 400 without a body drain or dispatch",
        raised.nil? && status == 400 && recvs == 1 && bodies.empty?,
        describe(*result)
      )
      check(
        "#{server}: a refused transfer coding closes the connection",
        raised.nil? && keep_alive == false && Sock.wire.out.include?("Connection: close\r\n"),
        describe(*result)
      )
    end
  end
  result = serve(server, post(2, "é".b))
  status, _recvs, bodies, raised, keep_alive = result
  check(
    "#{server}: an ordinary Content-Length body still serves",
    raised.nil? && status == 200 && keep_alive && bodies.map(&:b) == ["é".b],
    describe(*result)
  )
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
