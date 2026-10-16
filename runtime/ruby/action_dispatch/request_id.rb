# frozen_string_literal: true
# The two response headers Rails' outermost request middleware add to
# every response the app answers: `ActionDispatch::RequestId`
# (`X-Request-Id`) and `Rack::Runtime` (`X-Runtime`). Both sit after
# `ActionDispatch::Static` in a Rails 8.1 stack, so a file served from
# public/ or the asset directory carries neither, while an error page
# the app answers (a 404 for an unknown route) carries both.
#
# RUBY-FAMILY ONLY: the strict targets serve through their own
# dispatchers. Called by both dispatchers that carry this file, once
# per request, around everything else: the CRuby overlay's
# `Main.run_rack` and the spinel scaffold's `Main.dispatch`.
module ActionDispatch
  # actionpack 8.1's ActionDispatch::RequestId under
  # `config.action_dispatch.request_id_header`'s default name.
  module RequestId
    HEADER = "X-Request-Id"

    # The id the response carries for a request whose `X-Request-Id`
    # header is `value` ("" when it has none): the client's own id when
    # it is present, with every character but word characters, `-` and
    # `@` removed and cut to 255 (`gsub(/[^\w\-@]/, "").first(255)`),
    # else a fresh UUID. Presence is checked before the cleanup, so an
    # id made only of other characters answers "", as in Rails.
    #
    # Byte by byte rather than a gsub: the header arrives as bytes, and
    # Ruby's `\w` keeps ASCII word characters only, so every non-ASCII
    # byte goes, whatever the encoding says.
    def self.make(value)
      return SecureRandom.uuid if blank?(value)
      out = +""
      i = 0
      n = value.bytesize
      while i < n && out.bytesize < 255
        b = value.getbyte(i)
        if (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || b == 95 || b == 45 || b == 64
          out << b.chr
        end
        i += 1
      end
      out
    end

    # ActiveSupport's `blank?` for a header value: empty or whitespace.
    def self.blank?(value)
      i = 0
      n = value.bytesize
      while i < n
        b = value.getbyte(i)
        return false unless b == 32 || (b >= 9 && b <= 13)
        i += 1
      end
      true
    end
  end

  # rack 3.2's Rack::Runtime: the seconds the app took, on the monotonic
  # clock, as a float with six decimals. Not reopened under `Rack`:
  # the CRuby lane loads the rack gem, whose Rack::Runtime is a class.
  module Runtime
    HEADER = "X-Runtime"

    def self.clock_time
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end

    def self.header_value(started)
      format("%0.6f", clock_time - started)
    end
  end
end
