# `Timeout.timeout` / `Timeout::Error` — Campfire's unfurl deadline and
# TimeLimitedVideoPreviewer#capture.
#
# CRuby/JRuby trees reach Ruby's own default gem through BUNDLED
# `require "timeout"` (`project::BUNDLED`). Spinel (and other strict
# targets with no stdlib timeout) take this port via `spinel_files`.
#
# The port mirrors the one-argument form the corpus writes:
#
#   Timeout.timeout(sec) { ... }
#
# A second message/exception-class argument is not modeled. Wall-clock
# is measured with `Process.clock_gettime(Process::CLOCK_MONOTONIC)`.
# The worker thread is cancelled with `Thread#kill` when the deadline
# passes — enough for unfurl / capture to abort rather than hang forever.

module Timeout
  class Error < RuntimeError
  end

  def self.timeout(sec)
    seconds = sec.to_f
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
    # Single write of the outcome when the block finishes (or raises), so
    # the waiter does not observe a half-filled slot mid-yield.
    done = []
    worker = Thread.new do
      begin
        value = yield
        done[0] = :ok
        done[1] = value
      rescue StandardError => e
        done[0] = :err
        done[1] = e
      end
      done
    end
    while done[0].nil?
      if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
        worker.kill
        raise Error, "execution expired"
      end
      Thread.pass
    end
    worker.join
    if done[0] == :err
      raise done[1]
    end
    done[1]
  end
end
