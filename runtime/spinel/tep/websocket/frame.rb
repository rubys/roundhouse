# Tep::WebSocket::Frame -- single-frame codec.
#
# Surface:
#   - Frame.new(fin, opcode, payload)             build for emit
#   - frame.encode_unmasked -> String             server-side emit bytes
#   - Frame.parse_from_buf(s, start, avail, max_payload)  parse a recv'd frame
#       returns a ParseResult (frame + bytes_consumed, OR an error code).
#
# Server-side emit: never masks (RFC 6455 §5.3 -- server MUST NOT
# mask). Client-side emit isn't shipped here; tep is server-shaped.
#
# Parse handles three length encodings (7-bit / 16-bit / 64-bit),
# the 4-byte mask key, and applies the mask to recover the plaintext
# payload. Returns a structural error code (close-code-shaped) for
# the family of malformed-frame cases that warrant a 1002 close:
#   - reserved bits set
#   - reserved opcode
#   - client frame not masked
#   - control frame payload > 125
#   - control frame fragmented
#
# and a 1009 close for a frame whose advertised payload exceeds
# `max_payload` (the caller's `Driver#max_frame_size`).
module Tep
  module WebSocket
    class Frame
      attr_accessor :fin, :opcode, :payload

      def initialize(fin, opcode, payload)
        @fin     = fin
        @opcode  = opcode
        @payload = payload
      end

      # Build the unmasked server-side wire bytes. Length-encoding
      # picks the smallest form that fits the payload. No mask.
      #
      # The header is assembled as an array of byte values and emitted
      # with `pack("C*")` rather than `String#<<`-ing each byte: the
      # 16-bit length form has a `0x00` high byte for any 126..255-byte
      # payload, so the header must be binary-safe (e.g. Action Cable's
      # ~140-byte confirm_subscription corrupts and never reaches the
      # browser otherwise). `pack` yields a length-tracked, binary-safe
      # string; `+ @payload` is likewise a NUL-safe concat. This is the
      # idiomatic way to build binary in Ruby — kept deliberately, not as
      # a workaround: spinel's old `String#<<` NUL-drop (matz/spinel#1479)
      # is now fixed, but `pack` stays clearer and doesn't couple the
      # codec to a particular spinel version.
      def encode_unmasked
        head = []
        b0 = (@fin ? 0x80 : 0x00) | (@opcode & 0x0f)
        head << b0

        # bytesize, not length: the wire header advertises BYTES; a char
        # count under-reports for any multi-byte payload and desyncs the
        # peer's parser (same class as the #1500 sendfile fix).
        plen = @payload.bytesize
        if plen <= 125
          head << plen
        elsif plen <= 65535
          head << 126
          head << ((plen >> 8) & 0xff)
          head << (plen & 0xff)
        else
          head << 127
          i = 7
          while i >= 0
            head << ((plen >> (i * 8)) & 0xff)
            i -= 1
          end
        end
        head.pack("C*") + @payload
      end

      # Convert a single byte value (0..255) to a 1-char String.
      def self.byte_to_chr(n)
        (n & 0xff).chr
      end

      # Parse one frame from `s` (a binary String of recv'd bytes, read
      # from `sp_net_recv_some(:binstr)`). `start` is the byte offset to
      # begin reading; `avail` is the count of valid bytes. Byte reads go
      # through `String#getbyte`, which is binary-safe (returns the 0..255
      # value at an index regardless of embedded NULs) — so the 16-bit
      # length high byte (0x00 for payloads 126..255) parses correctly.
      #
      # `max_payload` is the largest payload this connection will accept —
      # `Driver#max_frame_size`, which the recv loop passes in.
      #
      # Returns a ParseResult with one of three shapes:
      #   .outcome == "ok"     -> .frame populated + .consumed bytes used
      #   .outcome == "need"   -> need more bytes (consumed == 0)
      #   .outcome == "close"  -> protocol violation; close with .close_code
      def self.parse_from_buf(s, start, avail, max_payload)
        out = Tep::WebSocket::ParseResult.new
        if avail - start < 2
          out.outcome = "need"
          return out
        end

        b0 = s.getbyte(start)
        b1 = s.getbyte(start + 1)
        fin    = (b0 & 0x80) != 0
        rsv    = b0 & 0x70
        opcode = b0 & 0x0f
        masked = (b1 & 0x80) != 0
        len7   = b1 & 0x7f

        if rsv != 0
          out.outcome = "close"
          out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
          return out
        end
        if Frame.reserved_opcode?(opcode)
          out.outcome = "close"
          out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
          return out
        end
        if Frame.control_opcode?(opcode)
          if !fin
            out.outcome = "close"
            out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
            return out
          end
          if len7 > 125
            out.outcome = "close"
            out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
            return out
          end
        end
        if !masked
          # Server MUST close on unmasked client frame (§5.3).
          out.outcome = "close"
          out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
          return out
        end

        # Decode payload length.
        pos = start + 2
        plen = 0
        if len7 < 126
          plen = len7
        elsif len7 == 126
          if avail - pos < 2
            out.outcome = "need"
            return out
          end
          h = s.getbyte(pos)
          l = s.getbyte(pos + 1)
          plen = (h << 8) | l
          pos += 2
        else
          # 64-bit length
          if avail - pos < 8
            out.outcome = "need"
            return out
          end
          # RFC 6455 §5.2: the most significant bit MUST be 0. Checked
          # before accumulating, because spinel's Integer is a fixed int64:
          # a set top bit overflows the loop's final `<<` — a raise inside
          # the recv loop, or on a wrapping build a negative length that
          # slips under the size guard below. Clear, the 8 bytes stay
          # under 2^63 and the loop cannot overflow.
          if s.getbyte(pos) >= 0x80
            out.outcome = "close"
            out.close_code = Tep::WebSocket::CLOSE_PROTOCOL_ERROR
            return out
          end
          plen = 0
          i = 0
          while i < 8
            plen = (plen << 8) | s.getbyte(pos + i)
            i += 1
          end
          pos += 8
        end

        # Oversize guard — 1009. This MUST come before the "need more
        # bytes" return below, and that ordering is the whole point of
        # it: `Connection#run` answers "need" by appending the next recv
        # to its accumulator and parsing again, so a frame we intend to
        # refuse must be refused from the LENGTH FIELD ALONE. Answering
        # "need" until the payload arrived would buffer every byte of
        # the payload the cap exists to reject — a 14-byte header
        # claiming a 64-bit length grew that accumulator until the
        # worker died, which is the OOM `DEFAULT_MAX_FRAME`'s comment
        # says it prevents. The cap was defined and settable but never
        # read; pinned by tests/spinel_websocket_frame_cap.rb.
        #
        # Control frames are already bounded at 125 above, and that
        # check runs first, so an oversized ping stays the 1002 it was.
        if plen > max_payload
          out.outcome = "close"
          out.close_code = Tep::WebSocket::CLOSE_MESSAGE_TOO_BIG
          return out
        end

        # 4-byte mask key.
        if avail - pos < 4
          out.outcome = "need"
          return out
        end
        m0 = s.getbyte(pos)
        m1 = s.getbyte(pos + 1)
        m2 = s.getbyte(pos + 2)
        m3 = s.getbyte(pos + 3)
        pos += 4

        # Payload bytes.
        if avail - pos < plen
          out.outcome = "need"
          return out
        end

        # Decode + unmask. Collect bytes into an int array and pack("C*")
        # rather than `<<`-ing chars — the idiomatic binary build, the
        # same reason encode_unmasked packs (see its note on #1479).
        bytes = []
        i = 0
        while i < plen
          b = s.getbyte(pos + i)
          mask_byte = 0
          if (i & 3) == 0
            mask_byte = m0
          elsif (i & 3) == 1
            mask_byte = m1
          elsif (i & 3) == 2
            mask_byte = m2
          else
            mask_byte = m3
          end
          bytes << (b ^ mask_byte)
          i += 1
        end
        payload = bytes.pack("C*")

        out.outcome   = "ok"
        out.frame    = Tep::WebSocket::Frame.new(fin, opcode, payload)
        out.consumed = pos + plen - start
        out
      end

      def self.reserved_opcode?(op)
        if op == Tep::WebSocket::OPCODE_CONTINUATION
          return false
        end
        if op == Tep::WebSocket::OPCODE_TEXT
          return false
        end
        if op == Tep::WebSocket::OPCODE_BINARY
          return false
        end
        if op == Tep::WebSocket::OPCODE_CLOSE
          return false
        end
        if op == Tep::WebSocket::OPCODE_PING
          return false
        end
        if op == Tep::WebSocket::OPCODE_PONG
          return false
        end
        true
      end

      def self.control_opcode?(op)
        op == Tep::WebSocket::OPCODE_CLOSE ||
          op == Tep::WebSocket::OPCODE_PING ||
          op == Tep::WebSocket::OPCODE_PONG
      end
    end

    # ParseResult carries either a parsed frame, a "need more
    # bytes" signal, or a close-code for a protocol violation.
    # Field is named `outcome` (not `status`) because attr_accessor
    # :status collides with Tep::Response.status (Integer) under
    # spinel's same-name-attr unification family
    # (matz/spinel#537 / #538), widening Tep.reason(status) to
    # accept poly and breaking the build.
    class ParseResult
      attr_accessor :outcome, :frame, :consumed, :close_code

      def initialize
        @outcome    = +""
        @frame      = Tep::WebSocket::Frame.new(true, 0, "")
        @consumed   = 0
        @close_code = 0
      end
    end
  end
end
