# Per-request state, per THREAD.
#
# Four modules in the shared runtime keep per-request state at module
# level: `ActionController::Current` (the request and controller the
# helpers read), `ActionView::ViewHelpers` (the `content_for` slots and
# the broadcast-render flag), `Broadcasts` (the log a test reads back),
# and `TypedStore` (a one-entry parse memo). `ActiveJob`'s queue is a
# plain Array written by requests and drained by a worker. All of that
# was correct under one cooperative fiber scheduler, where a request ran
# from start to finish before the next began, and every dispatch reset
# it. It is not correct under the threaded server: two requests served
# at once would share one `Current.controller`, one slot store, one log.
#
# This file REOPENS those modules for the Ruby-family lanes and moves
# the state into `Thread.current[...]` (fiber-local storage, which under
# spinel's green threads and under Puma's request threads alike means
# per request), or behind a Mutex where the state is meant to be shared.
# The shared definitions stay as they are for the strict targets, which
# transpile the shared runtime and have no threads. Required from boot.rb
# after every module it reopens; a later definition wins, on spinel as on
# CRuby.
#
# spinel types thread-local storage by key from its assignments, the way
# it types ivars, so each key below is written from exactly one shape.

module ActionController
  module Current
    def self.request
      Thread.current[:ac_current_request]
    end

    def self.request=(value)
      Thread.current[:ac_current_request] = value
    end

    def self.controller
      Thread.current[:ac_current_controller]
    end

    def self.controller=(value)
      Thread.current[:ac_current_controller] = value
    end

    def self.session
      c = Thread.current[:ac_current_controller]
      return nil if c.nil?
      c.session
    end
  end
end

module ActionView
  module ViewHelpers
    # The request's slot store, created on first touch.
    def self.__slots
      s = Thread.current[:view_slots]
      if s.nil?
        s = {}
        Thread.current[:view_slots] = s
      end
      s
    end

    def self.reset_slots!
      Thread.current[:view_slots] = {}
      Thread.current[:view_broadcast_rendering] = false
    end

    def self.content_for_set(slot, value)
      prior = get_slot(slot)
      __slots[slot] = prior + value
      nil
    end

    def self.content_for_get(slot)
      __slots.fetch(slot, nil)
    end

    def self.get_slot(slot)
      __slots[slot] || ""
    end

    def self.get_yield
      __slots[:__body__] || ""
    end

    def self.set_yield(content)
      __slots[:__body__] = content
      nil
    end

    def self.csrf_token_hidden_input
      return "" if Thread.current[:view_broadcast_rendering] == true
      %(<input type="hidden" name="authenticity_token" value="#{form_authenticity_token}">)
    end

    def self.begin_broadcast_render
      Thread.current[:view_broadcast_rendering] = true
      ""
    end

    def self.broadcast_render(_armed, html)
      Thread.current[:view_broadcast_rendering] = false
      html
    end
  end
end

module Broadcasts
  # The request's broadcast log, created on first touch.
  def self.__log
    l = Thread.current[:broadcast_log]
    if l.nil?
      l = []
      Thread.current[:broadcast_log] = l
    end
    l
  end

  def self.reset_log!
    Thread.current[:broadcast_log] = []
    nil
  end

  def self.log
    __log.dup
  end

  # One entry onto this request's log. `ActionCable::Server#pubsub`
  # writes here too, so a reader of `log` sees what a test's channel
  # published beside what a model broadcast.
  def self.log_append(entry)
    __log << entry
    nil
  end

  def self.record(action:, stream:, target:, html:, attributes: "")
    entry = { action: action, stream: stream, target: target, html: html, attributes: attributes }
    log_append(entry)
    fragment = render_fragment(action: action, target: target, html: html, attributes: attributes)
    TRANSPORTS[0].broadcast(stream, fragment)
    nil
  end
end

module ActiveJob
  # The performed log, per thread. Every job class's `perform_later`
  # records its name here BEFORE enqueueing -- on the request thread --
  # and only the test helpers read it, as a length delta across their
  # own block, on that same thread. The shared `PERFORMED` Array was the
  # one global string array the emit pushes onto at request time with no
  # lock: two workers' pushes tore it, and the next stop-the-world mark
  # walked the torn buffer (SIGSEGV in sp_StrArray_scan under
  # sp_gc_mark_drain, 2026-09-02). A per-thread log also stops the
  # server growing that Array for as long as it lives.
  def self.__performed
    l = Thread.current[:jobs_performed]
    if l.nil?
      l = []
      Thread.current[:jobs_performed] = l
    end
    l
  end

  def self.record_performed(job_name)
    __performed << job_name
    nil
  end

  def self.performed
    __performed.dup
  end

  # Requests enqueue from their own threads and one worker drains: the
  # Array is shared on purpose, so it is written under a lock. `drain`
  # takes one job out under the lock and runs it outside, so a slow job
  # never holds the queue against the requests still enqueueing.
  QUEUE_LOCK = Mutex.new

  def self.enqueue(work)
    QUEUE_LOCK.synchronize do
      PENDING << work
    end
    nil
  end

  def self.pending_count
    n = 0
    QUEUE_LOCK.synchronize do
      n = PENDING.length
    end
    n
  end

  def self.drain
    ran = 0
    while true
      work = nil
      QUEUE_LOCK.synchronize do
        work = PENDING.shift if PENDING.length > 0
      end
      break if work.nil?
      begin
        work.call
        ran = ran + 1
      rescue StandardError => e
        warn "[job] a queued job raised: " + e.message
      end
    end
    ran
  end
end

module TypedStore
  # The one-entry parse memo, per thread: the same row's ~24 attribute
  # reads still parse once, and two requests parsing different rows no
  # longer evict each other -- or hand each other the wrong hash.
  def self.parse(serialized)
    s = serialized.to_s
    cached_key = Thread.current[:typed_store_key]
    cached = Thread.current[:typed_store_val]
    return cached if !cached_key.nil? && s == cached_key && !cached.nil?
    h = {}
    lines = s.split("\n")
    i = 0
    while i < lines.length
      line = lines[i].to_s
      i += 1
      next if line == "---" || line == ""
      c0 = line[0, 1].to_s
      next if c0 == " " || c0 == "-" || c0 == "#"
      sep = line.index(": ")
      if sep.nil?
        if line.end_with?(":")
          h[line[0, line.length - 1].to_s] = nil
        end
        next
      end
      key = line[0, sep].to_s
      raw = line[sep + 2, line.length].to_s
      h[key] = if raw == "true"
        true
      elsif raw == "false"
        false
      elsif raw == raw.to_i.to_s
        raw.to_i
      elsif raw.length >= 2 && ((raw[0, 1] == "\"" && raw[raw.length - 1, 1] == "\"") ||
                                (raw[0, 1] == "'" && raw[raw.length - 1, 1] == "'"))
        raw[1, raw.length - 2].to_s
      else
        raw
      end
    end
    Thread.current[:typed_store_key] = s
    Thread.current[:typed_store_val] = h
    h
  end

  def self.write(serialized, key, value)
    h = {}
    parse(serialized).each do |k, v|
      h[k] = v
    end
    h[key] = value
    Thread.current[:typed_store_key] = nil
    out = "---\n"
    h.each do |k, v|
      out += k.to_s + ": " + (v.nil? ? "" : v.to_s) + "\n"
    end
    out
  end
end
