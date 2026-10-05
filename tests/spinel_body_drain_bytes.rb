# Driver for tests/spinel_body_drain_bytes.rs — the request-body drains
# in the spinel lane's HTTP server count BYTES.
#
# Run under plain CRuby against the real servers over a scripted socket
# (tests/tep_server_harness.rb), in its `utf8:` stress mode: recv'd
# chunks come back tagged UTF-8, so `String#length` counts characters on
# anything built from them. Under CRuby's binary chunks `length` equals
# `bytesize` and no char/byte confusion is visible at all.
#
# Content-Length counts bytes. 5cb6d051 moved the two drains in
# request.rb to `bytesize` after `raw_body.length` against a byte count
# cost spinel a 5s wait per multibyte body, but the blocking server's
# drain, `Sock.sphttp_drain_body` in net.rb, still compared `out.length`
# against the byte count it was handed. It measured correctly only
# because `out = out + chunk` keeps recv'd bytes binary on spinel
# (probed on 775ba5f68 — the same bytes appended with `<<` count
# characters). Had `out` counted characters, a multibyte body would have
# looked short with every byte in hand, and the loop would have read the
# head of the NEXT pipelined request into this request's body. That is
# the failure this stress mode reproduces and the fix rules out.

require_relative "tep_server_harness"

# ~12 KiB of two-byte characters: larger than the 4 KiB header read, so
# the drain does real work, and 6,000 characters short of its byte count.
BODY = ("title=" + "é" * 6000).b
FIRST = post(BODY.bytesize, BODY).b
SECOND = "GET /next HTTP/1.1\r\nHost: localhost\r\n\r\n".b

SERVERS.each_key do |s|
  # All of it in the drain's first recv, and then in 1,000-byte pieces —
  # which also splits characters across recvs.
  [[1 << 20, "in one piece"], [1000, "in 1000-byte pieces"]].each do |chunk, how|
    r = serve(s, FIRST + SECOND, utf8: true, chunk: chunk)
    status, _recvs, bodies, raised = r
    read = Sock.wire.pos
    check(
      "#{s}: a multibyte body #{how} reaches the app as exactly its declared bytes",
      raised.nil? && status == 200 && bodies.map(&:b) == [BODY],
      describe(*r)
    )
    check(
      "#{s}: a multibyte body #{how} leaves the next pipelined request unread",
      raised.nil? && read == FIRST.bytesize,
      "read #{read - FIRST.bytesize} byte(s) past the end of the request " \
      "(#{SECOND.byteslice(0, [read - FIRST.bytesize, 0].max).inspect})"
    )
  end
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
