# CRuby-only ActionView number helpers: `number_with_precision`.
#
# Lives on the CRuby overlay, not the shared runtime: the natural body is
# a `format("%.Nf")` round-trip, and neither Kernel#format nor a
# float-rounding surface exists uniformly across the transpiled targets
# (`**`/`Float#round`/`Integer#times` each broke a strict target when
# this sat in runtime/ruby — kotlin/go/rust/C#/elixir CI). Rounding-only
# subset (the users/show karma display); delimiter/separator options
# aren't modeled. When lobsters comes up on another target, that target
# grows its own number-helper surface. Callers reach this as
# `ActionView::ViewHelpers.number_with_precision(...)` via
# `apply_helper_lowering`'s framework-helper rewrite.
module ActionView
  module ViewHelpers
    # `number_with_delimiter(12345)` -> "12,345" (default delimiter).
    def self.number_with_delimiter(value, delimiter: ",")
      int, frac = value.to_s.split(".")
      out = +""
      int.chars.reverse.each_with_index do |c, i|
        out << delimiter if i > 0 && (i % 3).zero? && c != "-"
        out << c
      end
      grouped = out.reverse
      frac ? "#{grouped}.#{frac}" : grouped
    end

    def self.number_with_precision(value, precision: 3)
      format("%.#{precision}f", value.to_f)
    end
  end

  # Rails-namespace mixin form: app lib classes `include
  # ActionView::Helpers::NumberHelper` (lobsters' TimeSeries chart
  # subclass). Instance-method wrappers over the module functions
  # above, so the include resolves and the mixed-in surface matches
  # Rails' instance-method contract.
  module Helpers
    module NumberHelper
      def number_with_delimiter(value, delimiter: ",")
        ActionView::ViewHelpers.number_with_delimiter(value, delimiter: delimiter)
      end

      def number_with_precision(value, precision: 3)
        ActionView::ViewHelpers.number_with_precision(value, precision: precision)
      end
    end
  end
end
