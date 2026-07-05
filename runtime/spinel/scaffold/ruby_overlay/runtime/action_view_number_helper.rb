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
    def self.number_with_precision(value, precision: 3)
      format("%.#{precision}f", value.to_f)
    end
  end
end
