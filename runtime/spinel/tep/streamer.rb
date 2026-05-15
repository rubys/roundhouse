# Tep::Streamer -- subclass and override #pump(out). The framework
# emits chunked-encoding headers, calls pump with a Stream writer,
# then emits the end-of-stream marker. Cooperative; pump runs to
# completion before the connection moves on.
#
# spinel can't pass blocks into the framework, so this is the
# subclass-equivalent of `stream do |out| ... end`. The translator
# recognises the do/end form and emits a Streamer subclass for you.
module Tep
  class Streamer
    def pump(out)
      # default no-op; subclasses override
      0
    end
  end

  # Per-request handle the user's `pump` writes to. Wraps the client
  # fd so each `out.write(s)` becomes one chunked frame.
  class Stream
    attr_accessor :fd

    def initialize(fd)
      @fd = fd
    end

    def write(s)
      Sock.sphttp_write_chunk(@fd, s)
      0
    end
  end
end
