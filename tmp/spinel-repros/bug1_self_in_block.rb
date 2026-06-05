# Spinel bug: a block passed to a method that declares an explicit `&block`
# parameter loses `self` when the block body calls another method on the
# implicit receiver. The lowered proc references `self` but the proc's C
# signature has no `self` param and captures NULL.
#
#   spinel bug1_self_in_block.rb -o /tmp/x
#   -> error: use of undeclared identifier 'self'
#
# Trigger  = method declares `&block` (vs bare `yield`) AND block calls a
#            method on implicit self.
# Controls = remove `&block` (bare yield) -> ok ; block with no self-call -> ok.

class W
  def label(n) = n
  def section(&block) = yield        # explicit &block param is the trigger
  def render = section do label("h2") end   # block calls self.label
end
puts W.new.render
