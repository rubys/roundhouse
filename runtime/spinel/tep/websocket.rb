# Tep::WebSocket -- RFC 6455 WebSocket support for spinel-AOT'd apps.
#
# Protocol substrate (this file's directory):
#   - Tep::WebSocket::Frame      single-frame codec (parse + emit)
#   - Tep::WebSocket::Handshake  server-side handshake check + response
#   - Tep::WebSocket::Driver     state machine + event dispatch + writers
#   - Tep::WebSocket::Connection fiber-driven recv loop (one fiber per conn)
#
# Sinatra-style DSL (lowered by bin/tep):
#
#     set :scheduler, :scheduled
#     websocket '/chat' do |ws|
#       on_open    { |evt| ws.text("welcome") }
#       on_message { |evt| ws.text("echo: " + evt.data) }
#       on_close   { |evt| ... }
#     end
#
# Requires the scheduled server (the recv loop parks on
# Tep::Scheduler.io_wait); the blocking server returns 501 on a WS
# upgrade attempt. See examples/websocket_echo.rb for a full app and
# test/test_websocket_echo.rb for the end-to-end smoke harness.
#
# Compliance posture (per the OriPekelman/tep#8 strict/lenient table):
#   strict-emit: server NEVER masks; reserved bits 0 on emit
#   strict-accept (close 1002):
#     - client frames MUST be masked
#     - reserved bits RSV1-3 MUST be 0
#     - reserved opcodes (3-7, B-F) reject
#     - control frame payload > 125 reject
#     - control frames MUST NOT fragment
#     - continuation without prior fragment reject
#   strict-accept (close 1007): text frames MUST be UTF-8 (deferred to
#     Phase 2.1 -- the codec ships the structural strictness first;
#     the UTF-8 validator is its own ~50 LOC).
#   liberal-accept: close codes, pong payload contents, unsolicited pong.
module Tep
  module WebSocket
    # Standard opcodes.
    OPCODE_CONTINUATION = 0
    OPCODE_TEXT         = 1
    OPCODE_BINARY       = 2
    OPCODE_CLOSE        = 8
    OPCODE_PING         = 9
    OPCODE_PONG         = 10

    # Close codes (RFC 6455 §7.4). Caller-facing ones only -- the
    # internal-error / protocol-error codes are emitted by the
    # Driver directly, not exposed.
    CLOSE_NORMAL          = 1000
    CLOSE_GOING_AWAY      = 1001
    CLOSE_PROTOCOL_ERROR  = 1002
    CLOSE_UNSUPPORTED     = 1003
    CLOSE_INVALID_UTF8    = 1007
    CLOSE_POLICY_VIOLATION = 1008
    CLOSE_MESSAGE_TOO_BIG = 1009

    # Frame-size cap. Configurable via Driver#set_max_frame_size;
    # default is 16 MiB (large enough for any realistic chat /
    # Action Cable payload, bounded so an oversized frame can be
    # closed with 1009 rather than OOM-ing the worker).
    DEFAULT_MAX_FRAME = 16 * 1024 * 1024
  end
end

require_relative "websocket/frame"
require_relative "websocket/handshake"
require_relative "websocket/driver"
require_relative "websocket/connection"
