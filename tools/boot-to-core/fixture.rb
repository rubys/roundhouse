# frozen_string_literal: true

# Deliberately unknown to the exporter: names, arithmetic and initial state
# are provided by a generator, not by compiler-side DSL rules.
module UnknownGenerator
  def self.evaluated(target, multiplier)
    target.class_eval("def compute(value)\n value * #{multiplier} + 7\nend", "generated.rb", 1)
    target.class_eval("def fallback(value)\n value || \"missing\"\nend", "generated.rb", 1)
  end

  def self.closures(target, prefix, counter, enabled)
    target.define_method(:label) { |id| "λ#{prefix}:#{id}" }
    target.define_method(:rename) { |replacement| prefix = replacement }
    target.define_method(:advance) { |amount| counter += amount }
    target.define_method(:current) { counter }
    target.define_method(:flag) { enabled }
    target.define_method(:selected) { |fallback| enabled || fallback }
  end
end

class EvalProduct; end
class LeftProduct; end
class RightProduct; end

UnknownGenerator.evaluated(EvalProduct, 3)
# Final override must win, rather than the first recorded definition.
EvalProduct.class_eval("def compute(value)\n value * 5 - 2\nend", "override.rb", 1)
UnknownGenerator.closures(LeftProduct, "north", 17, false)
UnknownGenerator.closures(RightProduct, "south", 101, nil)
# Export the actual post-boot slot value, not the factory's initial input.
LeftProduct.new.advance(2)
