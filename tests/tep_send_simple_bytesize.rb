# A refusal's Content-Length counts the bytes it writes, on all three servers.
#
# write_response already frames in bytes; send_simple (400/413/500 and the
# fiber lane's 501) still measured its message with String#length, which
# counts characters. Every message it is handed today is ASCII, so the two
# agree -- the first non-ASCII message (a localized reason, say) would
# announce fewer bytes than go on the wire, and a keep-alive client reads
# the surplus as the start of the next response.
require_relative "tep_server_harness"

MSG = "requisição inválida" # 19 characters, 22 bytes
check("probe distinguishes bytes from characters", MSG.bytesize != MSG.length)

SENDERS = {
  "threaded" => ->(fd) { Tep::Server::Threaded.send_simple(fd, 400, MSG) },
  "scheduled" => ->(fd) { Tep::Server::Scheduled.send_simple(fd, 400, MSG) },
  "blocking" => ->(fd) { Tep::Server.new(APP).send_simple(fd, 400, MSG) }
}.freeze

SENDERS.each do |server, send|
  Sock.wire = Wire.new("")
  send.call(7)
  head, body = Sock.wire.out.split("\r\n\r\n", 2)
  declared = head[/^Content-Length: (\d+)/i, 1].to_i
  check(
    "#{server}: send_simple's Content-Length is the body's byte count",
    declared == body.bytesize,
    "declared #{declared}, wrote #{body.bytesize} bytes " \
    "(#{body.dup.force_encoding(Encoding::UTF_8).length} characters)"
  )
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
