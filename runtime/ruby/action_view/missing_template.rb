# ActionView::MissingTemplate — raised by the render rewrite when a
# render target was never emitted (the template doesn't exist in the
# source tree), and RESCUED by app code as a normal path: lobsters'
# about/privacy/chat serve hardcoded fallbacks from that rescue.
# Ruby-family home (off the strict-target tables): exception classes
# as control flow ride the BeginRescue lowering, which the ruby-family
# targets and spinel AOT share; strict targets get their own story
# when their lobsters turn comes.
module ActionView
  class MissingTemplate < StandardError
  end
end
