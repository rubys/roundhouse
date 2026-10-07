# frozen_string_literal: true

# Per-page view buffer capacity memo (CRuby overlay).
#
# Ports keep the last render size in `Ractor[:cap_<page>]` and allocate
# `String.new(capacity: N)`. Here the same idea uses thread variables —
# Puma threads learn independently, and the memo survives fiber-per-request
# (`Thread.current[]` is fiber-local). A torn Integer size across threads
# is harmless (capacity is a hint; an underestimate still produces correct
# HTML as the string grows).
#
# Spinel's copy of this path is a no-op stub (`String.new` / ignore store)
# so AOT never sees the `capacity:` keyword.
module ViewBufferCap
  def self.alloc(key)
    n = Thread.current.thread_variable_get(key)
    if n.nil?
      String.new
    else
      String.new(capacity: n)
    end
  end

  def self.store(key, size)
    Thread.current.thread_variable_set(key, size)
    nil
  end
end
