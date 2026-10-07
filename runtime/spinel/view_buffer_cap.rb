# frozen_string_literal: true

# Spinel stub: returning view wrappers call these (see
# `lower::view_buffer_passing`). Capacity pre-sizing is a CRuby overlay
# concern — Spinel Strings grow without a `capacity:` keyword, and this
# stub keeps AOT free of that surface. The CRuby overlay replaces this
# file with a thread-variable memo (ports use `Ractor[:cap_<page>]`).
module ViewBufferCap
  def self.alloc(_key)
    String.new
  end

  def self.store(_key, _size)
    nil
  end
end
