# frozen_string_literal: true
# `allow_browser`'s verdict once per User-Agent per thread, not once per
# request. The shared BrowserBlocker.blocked? parses the header every
# time, as Rails' own does: 28.7 us and 124 objects for a current Chrome
# agent on rubix2 (2026-10-22), on every request through a controller that
# gates browsers, against 9 us for the hand-ported campfire's whole /up.
# The verdict is a pure function of the header and the floors table, so it
# is kept, keyed by both, in a bounded per-thread table. Keyed by the
# floors' CONTENTS: the lowered `allow_browser` passes its table as a
# literal, a new Hash on every request (a thread
# variable, not `Thread.current[]`, which is per fiber: under Falcon, per
# request). A full table is cleared rather than evicted from: agents
# repeat, and a client cycling through more of them than the cap pays the
# parse it would have paid anyway.
module ActionController
  module BrowserBlocker
    VERDICT_CACHE_CAP = 1024

    class << self
      alias_method :blocked_uncached?, :blocked?

      def blocked?(user_agent, floors)
        return false if user_agent.nil? || user_agent.empty?
        thread = Thread.current
        tables = thread.thread_variable_get(:roundhouse_browser_verdicts) ||
          thread.thread_variable_set(:roundhouse_browser_verdicts, {})
        table = tables[floors]
        if table.nil?
          tables.clear if tables.size >= 16
          table = tables[floors.dup.freeze] = {}
        end
        verdict = table[user_agent]
        return verdict unless verdict.nil?
        table.clear if table.size >= VERDICT_CACHE_CAP
        table[user_agent.frozen? ? user_agent : user_agent.dup.freeze] = blocked_uncached?(user_agent, floors)
      end
    end
  end
end
