require_relative "tep_core"
require_relative "url"
require_relative "net"
require_relative "streamer"
require_relative "request"
require_relative "response"
require_relative "parser"
require_relative "server"

module Tep
  # Type-seeding: pin parameter types for transport methods that
  # roundhouse's dispatch may not exercise from every angle. Session
  # was removed from the vendored copy (collides with controllers'
  # :session ivar via poly dispatch); ActionDispatch::Session takes
  # its place.
  _tep_seed_res = Response.new
  _tep_seed_res.set_cookie("", "", str_hash)
  _tep_seed_res.start_stream(Streamer.new)
  _tep_seed_stream = Stream.new(0)
  _tep_seed_res.streamer.pump(_tep_seed_stream)
  _tep_seed_stream.write("")
end
