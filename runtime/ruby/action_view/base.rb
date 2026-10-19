# frozen_string_literal: true

# The view context Rails instantiates for a template. Roundhouse still
# macro-expands `form_with` into the accumulator (the block binds a
# typed builder the runtime does not reconstruct), but each expanded
# site begins by calling this instance method. A fragment-cache hit
# never walks that site, so it never enters `form_with`.
#
# That is the entry `ActionView::Base.any_instance.expects(:form_with)`
# observes. The method returns nil; the markup that follows in the
# expansion is the form. A body that also emitted the tags would
# double them.
module ActionView
  class Base
    def form_with
      nil
    end
  end
end
