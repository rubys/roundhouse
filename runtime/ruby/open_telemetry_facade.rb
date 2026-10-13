# OpenTelemetry — `OpenTelemetry::Trace.current_span` stands behind
# `deprecate_api`-shaped concern macros (a `before_action` tail that
# tags the current span as deprecated when one is recording). NOT a
# `GemFacade.fail!` stand-in like this file's other occupants: those
# model write-path gem surface the read benchmark never reaches, where
# a raise turns "got here unexpectedly" into a visible failure. This
# module is read-path (every request to a deprecated endpoint reaches
# it) and its behavior here is not a stand-in for something missing —
# it IS what `opentelemetry-api` itself answers when no SDK is
# installed: a non-recording span, so `recording?` is false and nothing
# downstream can tell a trace was never started. Keeping that real
# no-SDK behavior, rather than raising, is what keeps a deprecated
# endpoint serving 200s instead of 500ing the moment its `before_action`
# reaches the OTel tail.
#
# Own file, own commit: a tree that later wires a real
# opentelemetry-api/-sdk pair drops this file and the one
# `require_relative` below, same whole-file grain the other façades
# that stand aside for a real gem use (bcrypt_facade.rb and friends) —
# though unlike those, nothing here EVER stands aside for a gem; the
# behavior below is permanent on every target, ruby family and Spinel
# alike, which is why `project.rs`'s per-flavor gem_facades rewrite
# requires this file unconditionally rather than leaving it to the
# guarded-require block the real stubbed gems go through.
module OpenTelemetry
  module Trace
    class Span
      def recording?
        false
      end

      def set_attribute(_key, _value)
        self
      end
    end

    NON_RECORDING_SPAN = Span.new

    def self.current_span
      NON_RECORDING_SPAN
    end
  end
end
