# Tep::WebSocket::Handshake -- RFC 6455 §1.3 server-side handshake.
#
# `check(req)`:
#   Returns a Result with `.valid` true if the request is a proper
#   WebSocket upgrade, `.accept_key` set to the Sec-WebSocket-Accept
#   value the server should echo, and `.protocols` parsed from
#   Sec-WebSocket-Protocol. Invalid uses set `.valid = false` +
#   `.reason` for logging.
#
# `build_response(accept_key, protocol)`:
#   Returns the raw HTTP/1.1 101 Switching Protocols response bytes,
#   ready to write to the socket. `protocol` is the subprotocol to
#   echo (empty string = omit the header per RFC §1.3 -- the safe
#   default per rubys's pushback on tep#8).
module Tep
  module WebSocket
    class Handshake
      def self.check(req)
        out = Tep::WebSocket::Handshake::Result.new

        # Verb must be GET.
        if req.verb != "GET"
          out.valid = false
          out.reason = "bad verb"
          return out
        end

        # Upgrade + Connection headers (downcased per Tep::Request).
        upgrade = req.req_headers["upgrade"]
        if Handshake.icontains(upgrade, "websocket") == false
          out.valid = false
          out.reason = "missing/invalid Upgrade"
          return out
        end
        conn = req.req_headers["connection"]
        if Handshake.icontains(conn, "upgrade") == false
          out.valid = false
          out.reason = "missing/invalid Connection"
          return out
        end

        # Sec-WebSocket-Version must be 13.
        ver = req.req_headers["sec-websocket-version"]
        if ver != "13"
          out.valid = false
          out.reason = "bad/missing Sec-WebSocket-Version"
          return out
        end

        # Sec-WebSocket-Key: 24-char base64 (16-byte nonce).
        key = req.req_headers["sec-websocket-key"]
        if key.length == 0
          out.valid = false
          out.reason = "missing Sec-WebSocket-Key"
          return out
        end

        out.valid = true
        out.accept_key = Crypto.sp_crypto_websocket_accept(key)

        # Parse Sec-WebSocket-Protocol (comma-separated). Handler
        # gets the offered list; can opt-in via Driver.accept_protocol.
        out.protocols = Handshake.split_csv(req.req_headers["sec-websocket-protocol"])
        out
      end

      # Build the 101 Switching Protocols response. `protocol` empty
      # = omit Sec-WebSocket-Protocol entirely (spec-correct per
      # RFC 6455 §4.2.2; better than echoing a protocol the server
      # doesn't actually implement).
      def self.build_response(accept_key, protocol)
        out = "HTTP/1.1 101 Switching Protocols\r\n" +
              "Upgrade: websocket\r\n" +
              "Connection: Upgrade\r\n" +
              "Sec-WebSocket-Accept: " + accept_key + "\r\n"
        if protocol.length > 0
          out = out + "Sec-WebSocket-Protocol: " + protocol + "\r\n"
        end
        out + "\r\n"
      end

      # Case-insensitive substring contains. Hand-rolled because
      # Tep::Request normalises header names to lowercase but
      # leaves values as-is, and clients sometimes send
      # `Connection: keep-alive, Upgrade` capitalised.
      def self.icontains(hay, needle)
        if hay.length == 0 || needle.length == 0
          return false
        end
        hl = Handshake.downcase(hay)
        nl = Handshake.downcase(needle)
        Tep.str_find(hl, nl, 0) >= 0
      end

      def self.downcase(s)
        out = ""
        i = 0
        while i < s.length
          c = s[i]
          if c >= "A" && c <= "Z"
            out = out + (c.ord + 32).chr
          else
            out = out + c
          end
          i += 1
        end
        out
      end

      # Parse comma-separated header value into an Array<String>.
      # Trims whitespace around each entry.
      def self.split_csv(s)
        out = [""]
        out.delete_at(0)
        if s.length == 0
          return out
        end
        pos = 0
        while pos < s.length
          comma = Tep.str_find(s, ",", pos)
          if comma < 0
            out.push(Handshake.trim(s[pos, s.length - pos]))
            return out
          end
          out.push(Handshake.trim(s[pos, comma - pos]))
          pos = comma + 1
        end
        out
      end

      def self.trim(s)
        i = 0
        while i < s.length && (s[i] == " " || s[i] == "\t")
          i += 1
        end
        j = s.length - 1
        while j >= i && (s[j] == " " || s[j] == "\t")
          j -= 1
        end
        if j < i
          return ""
        end
        s[i, j - i + 1]
      end

      class Result
        attr_accessor :valid, :reason, :accept_key, :protocols

        def initialize
          @valid      = false
          @reason     = ""
          @accept_key = ""
          @protocols  = [""]
          @protocols.delete_at(0)
        end
      end
    end
  end
end
