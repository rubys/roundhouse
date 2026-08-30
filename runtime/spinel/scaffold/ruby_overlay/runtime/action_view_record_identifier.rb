# `ActionView::RecordIdentifier` — where Rails actually DEFINES `dom_id`.
#
# `ActionView::ViewHelpers.dom_id` is our home for it; Rails' is this
# module, which its helpers `include` so a template gets a bare `dom_id`.
# App code that reaches the module BY NAME rather than through a view is
# what needs it — campfire's `turbo_test_helper` writes
#
#   target = ActionView::RecordIdentifier.dom_id(*target)
#
# to build the DOM id it then asserts a broadcast was targeted at.
#
# A DELEGATION rather than a move: Rails has the ownership the other way
# round, and flipping ours to match would rewrite every existing call
# site to buy nothing. One spelling of the logic, and it stays in
# `dom_id`.
#
# WHY THIS IS AN OVERLAY FILE and not `runtime/ruby/`, which is where
# `dom_id` itself lives. That directory prices all nine targets, and the
# ones that have no module system flatten `ActionView`'s modules into a
# single namespace — Kotlin emitted both `domId`s into `ViewHelpers.kt`
# and the build failed with "Conflicting overloads", then "Overload
# resolution ambiguity" at the call site. Nothing outside the ruby family
# names this constant, so the fix is to stop offering it to targets that
# cannot express it. The `Ty::Untyped` ratchet in
# `tests/runtime_src_integration.rs` caught the first version of this
# same change for a different reason; between them the two gates are what
# stop `runtime/ruby/` growing surface it should not carry.
#
# `dom_id` alone: `dom_class` and `RecordIdentifier::JOIN` are the
# module's other exports and no ingested app has named either.
module ActionView
  module RecordIdentifier
    def self.dom_id(record, suffix = nil)
      ActionView::ViewHelpers.dom_id(record, suffix)
    end
  end
end
