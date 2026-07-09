# CRuby-only ActionView::MissingTemplate.
#
# Lobsters renders templates that don't exist in the source tree
# (about/privacy/chat) and rescues ActionView::MissingTemplate to serve
# a hardcoded fallback — that rescue-as-control-flow is the page's
# NORMAL path. The render rewrite emits `raise ActionView::
# MissingTemplate` when the target view was never emitted, and the app's
# own rescue handles it. Overlay, not shared runtime: exception classes
# as control flow are a CRuby-dynamic idiom; strict targets get their
# own story when lobsters comes up on them.
module ActionView
  class MissingTemplate < StandardError
  end
end
