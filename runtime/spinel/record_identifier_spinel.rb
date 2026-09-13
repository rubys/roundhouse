# `ActionView::RecordIdentifier.dom_id` for the spinel tree — the CRuby
# overlay's `action_view_record_identifier.rb`, whose header says why the
# module lives beside the shared runtime and not in it (targets that
# flatten `ActionView`'s modules emitted two `domId`s). Same delegation,
# same one spelling of the logic: `ViewHelpers.dom_id`.
#
# campfire's `turbo_test_helper` reaches the module by name —
# `ActionView::RecordIdentifier.dom_id(*target)` — to build the DOM id it
# asserts a broadcast was targeted at. Loaded from spinel's boot.rb only,
# after action_controller's chain has defined the shared `dom_id`.
module ActionView
  module RecordIdentifier
    def self.dom_id(record, suffix = nil)
      ActionView::ViewHelpers.dom_id(record, suffix)
    end
  end
end
