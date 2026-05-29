# Tep::WebSocket::Frame -- single-frame codec.
#
# Surface:
#   - Frame.new(fin, opcode, payload)             build for emit
#   - frame.encode_unmasked -> String             server-side emit bytes
#   - Frame.parse_from_buf(bytes_at, bytes_len)   parse a recv'd frame
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
      def encode_unmasked
        head = ""
        b0 = (@fin ? 0x80 : 0x00) | (@opcode & 0x0f)
        head = head + Frame.byte_to_chr(b0)

        plen = @payload.length
        if plen <= 125
          head = head + Frame.byte_to_chr(plen)
        elsif plen <= 65535
          head = head + Frame.byte_to_chr(126)
          head = head + Frame.byte_to_chr((plen >> 8) & 0xff)
          head = head + Frame.byte_to_chr(plen & 0xff)
        else
          head = head + Frame.byte_to_chr(127)
          i = 7
          while i >= 0
            head = head + Frame.byte_to_chr((plen >> (i * 8)) & 0xff)
            i -= 1
          end
        end
        head + @payload
      end

      # Convert a single byte value (0..255) to a 1-char String.
      def self.byte_to_chr(n)
        (n & 0xff).chr
      end

      # Parse one frame from the sphttp recv frame buffer. `start`
      # is the byte offset to begin reading; `avail` is the count of
      # valid bytes in the buffer. Byte reads go through the Ruby
      # String binding sphttp_recv_frame_buf returns; matz/spinel#657
      # made slice / bytes[i] survive embedded NULs so binary
      # payloads parse correctly without the per-byte C accessor we
      # used before.
      #
      # Returns a ParseResult with one of three shapes:
      #   .status == "ok"      -> .frame populated + .consumed bytes used
      #   .status == "need"    -> need more bytes (consumed == 0)
      #   .status == "close"   -> protocol violation; close with .close_code
      def self.parse_from_buf(start, avail)
        out = Tep::WebSocket::ParseResult.new
        if avail - start < 2
          out.outcome = "need"
          return out
        end

        # Read bytes through the per-byte C accessor rather than
        # `sphttp_recv_frame_buf.bytes`: the :str accessor is NUL-
        # terminated on the Ruby side, so a masked frame truncates at
        # its first 0x00 (the 16-bit length high byte is 0x00 for any
        # payload <= 255 bytes). See sphttp.c / net.rb.
        b0 = Sock.sphttp_recv_frame_byte(start)
        b1 = Sock.sphttp_recv_frame_byte(start + 1)
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
          h = Sock.sphttp_recv_frame_byte(pos)
          l = Sock.sphttp_recv_frame_byte(pos + 1)
          plen = (h << 8) | l
          pos += 2
        else
          # 64-bit length
          if avail - pos < 8
            out.outcome = "need"
            return out
          end
          plen = 0
          i = 0
          while i < 8
            plen = (plen << 8) | Sock.sphttp_recv_frame_byte(pos + i)
            i += 1
          end
          pos += 8
        end

        # 4-byte mask key.
        if avail - pos < 4
          out.outcome = "need"
          return out
        end
        m0 = Sock.sphttp_recv_frame_byte(pos)
        m1 = Sock.sphttp_recv_frame_byte(pos + 1)
        m2 = Sock.sphttp_recv_frame_byte(pos + 2)
        m3 = Sock.sphttp_recv_frame_byte(pos + 3)
        pos += 4

        # Payload bytes.
        if avail - pos < plen
          out.outcome = "need"
          return out
        end

        # Decode + unmask in one pass.
        payload = ""
        i = 0
        while i < plen
          b = Sock.sphttp_recv_frame_byte(pos + i)
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
          payload = payload + Frame.byte_to_chr(b ^ mask_byte)
          i += 1
        end

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
        @outcome    = ""
        @frame      = Tep::WebSocket::Frame.new(true, 0, "")
        @consumed   = 0
        @close_code = 0
      end
    end
  end
end
