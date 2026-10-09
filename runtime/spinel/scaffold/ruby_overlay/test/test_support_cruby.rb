# Test support the CRuby tree can give and the spinel tree cannot:
# loaded last by test/test_helper.rb on the ruby target only
# (`project.rs::apply_cruby_test_support`).
#
# Rails' `ActiveSupport::Testing::ConstantStubbing#stub_const`, as Rails
# writes it: the constant is REPLACED for the block and put back after,
# so code that reads it at call time (`Message::Pagination::PAGE_SIZE`,
# `Opengraph::Fetch::TIMEOUT`) sees the stub. A compiled tree resolves
# constants at build time and has no `const_set`; there the helper is
# absent and the call fails as the gap it is.
class TestBase
  def stub_const(mod, constant, new_value)
    old_value = mod.const_get(constant, false)
    begin
      mod.send(:remove_const, constant)
      mod.const_set(constant, new_value)
      yield
    ensure
      mod.send(:remove_const, constant) if mod.const_defined?(constant, false)
      mod.const_set(constant, old_value)
    end
  end
end

# Rails-dom-testing's `css_select`: the elements of the last response
# matching `selector`, as Nokogiri nodes (`node["id"]`). Rails' own is
# Nokogiri; the shared harness's `Dom` is a scanner that matches a
# selector's LAST compound and keeps no per-element attributes, so a
# descendant selector read from it would answer for the whole page.
require "nokogiri"

class TestBase
  def css_select(selector)
    Nokogiri::HTML5.fragment(@__response.body.to_s).css(selector).to_a
  end
end

# ActionCable::TestHelper#assert_broadcast_on: some broadcast to
# `stream` — during the block, or so far in this test without one —
# carries `data`, compared as Rails compares it, after a JSON round
# trip (symbol keys and all become what the wire carries).
module ActionCable
  module TestHelper
    def assert_broadcast_on(stream, data, &block)
      entries =
        if block
          capture_broadcasts_on(stream, &block)
        else
          Broadcasts.log.select { |entry| entry[:stream] == stream }
        end
      # The CRuby tree logs the Hash it publishes, the spinel tree the
      # JSON text; both compare after the same round trip.
      wire = ->(value) { ::JSON.parse(value.is_a?(String) ? value : ::JSON.generate(value)) }
      want = wire.(data)
      payloads = entries.map { |entry| entry[:payload] }.compact
      return if payloads.any? { |payload| wire.(payload) == want }
      raise "assert_broadcast_on failed: no broadcast to #{stream.inspect} carried #{want.inspect}; " \
            "got #{payloads.inspect}"
    end
  end
end
