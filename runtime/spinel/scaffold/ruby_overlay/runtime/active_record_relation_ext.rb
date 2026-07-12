# CRuby-only ActiveRecord::Relation extensions: enumerable terminals
# and record-valued IN lists.
#
# Lives on the CRuby overlay, not shared runtime/ruby/active_record/
# relation.rb: the IN-list coercion dispatches on
# is_a?(ActiveRecord::Base) — exactly the shape the typed shared
# runtime refuses (see the monomorphic-API rule). When lobsters comes
# up on another target, that target grows its own variant.
# (`Relation#+` lived here too until it moved to the shared runtime.)
module ActiveRecord
  class Relation
    # Enumerable terminal Rails relations answer via to_a delegation;
    # lobsters' Comment.arrange_for_user groups its ordered relation by
    # parent id.
    def group_by(&block)
      to_a.group_by(&block)
    end

    private

    # Rails casts an ActiveRecord object used as a condition value to its
    # id (`where(story_id: [<Story rows>, 7])` → `IN (3, 5, 7)`). The
    # shared escape_list feeds values straight to the adapter, which only
    # knows scalars — so the record→id cast happens here, on the overlay.
    def escape_list(vals)
      out = []
      vals.each do |v|
        v = v.id if v.is_a?(ActiveRecord::Base)
        out << ActiveRecord.adapter.escape_value(v)
      end
      out.join(", ")
    end
  end
end
