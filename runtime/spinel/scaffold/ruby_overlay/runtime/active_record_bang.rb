# CRuby-only ActiveRecord::Base raising variants: `find_by!`.
#
# Lives on the CRuby overlay, not shared runtime/ruby/active_record/base.rb:
# the shared Base transpiles method-by-method into every target's model
# modules, and the strict targets don't carry a generic `find_by/1` for it
# to call (elixir CI: `undefined function find_by/1` in every model).
# Class-method inheritance makes this reach every model on CRuby. When
# lobsters comes up on another target, that target grows its own variant.
module ActiveRecord
  class Base
    def self.find_by!(conditions)
      result = find_by(conditions)
      raise RecordNotFound, "Couldn't find #{name}" if result.nil?
      result
    end
  end
end
