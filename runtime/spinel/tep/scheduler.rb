# Tep::Scheduler -- a tiny fiber-based cooperative scheduler.
#
# Spinel ships Fiber today (ucontext-based, GC-aware, ivars persist
# across yields). What was missing was the layer above: a way to run
# multiple cooperating fibers within a single worker process so a
# long-running response (SSE stream, long-poll, slow batch) doesn't
# pin the worker for the whole connection lifetime.
#
# This covers two parking modes:
#
#   * **Time**: register a fiber to be resumed at-or-after `wake_at`
#     via `Tep::Scheduler.pause(seconds)`.
#   * **I/O**: park a fiber on (fd, mode) via `Tep::Scheduler.io_wait`.
#     tick() runs a poll(2) round, marks ready fibers, and resumes them
#     (along with any time-ready ones) on the same pass.
#
# Storage shape
# -------------
# Parallel arrays on the Tep::APP singleton -- one entry per fiber:
#   sched_fibers    PtrArray<FiberSlot>  the Fiber itself
#   sched_wake_at   IntArray             unix-seconds; -1 = ready now
#   sched_io_fd     IntArray             fd parked on; -1 = no I/O wait
#   sched_io_mode   IntArray             requested mode bits (1=R, 2=W)
#   sched_io_ready  IntArray             observed-ready bits (0=not yet)
#
# Spinel handles same-shaped typed arrays cleanly; using a single
# array of structs would force a poly_array. Same App-instance
# pattern as Tep::Assets.
#
# What it doesn't do (yet)
# ------------------------
# **Implicit yield on blocking calls.** Ruby 3.0's
# `Fiber::SchedulerInterface` makes every blocking I/O auto-yield
# to a registered scheduler. Spinel doesn't recognise that hook;
# fibers yield explicitly via `Tep::Scheduler.pause / io_wait`.
#
# **Non-blocking accept on the listening socket.** The Server's
# worker_loop still does a blocking accept(); fibers cooperate
# *within* a single request lifetime, not across requests. Adding
# poll-on-accept needs the worker_loop to opt into the scheduler.
module Tep
  class Scheduler
    # Mode bits for io_wait. Mirror sphttp's wire encoding so the
    # C side and Ruby side stay aligned.
    READ  = 1
    WRITE = 2

    def self.spawn_fiber(f)
      Tep::APP.sched_fibers.push(Tep::FiberSlot.new(f))
      Tep::APP.sched_wake_at.push(-1)
      Tep::APP.sched_io_fd.push(-1)
      Tep::APP.sched_io_mode.push(0)
      Tep::APP.sched_io_ready.push(0)
      f
    end

    # One scheduler pass. If any fibers are parked on I/O, build a
    # poll set, run poll(2) for up to `poll_timeout_ms`, and mark
    # ready ones. Then resume the soonest-due fiber whose wake_at
    # is <= now. Returns true if it resumed something.
    #
    # If a fiber is already time-due (wake_at <= now -- e.g. a newly
    # spawned fiber with wake_at=-1, or a fiber that just called
    # pause(0)), poll() must NOT block: we have runnable work and
    # any wait is wasted wall time. This matters for the cooperative
    # request path -- when an outer handler parks on io_wait and
    # the accept fiber spawns an inner connection-fiber, the next
    # tick has a wake_at=-1 fiber ready; without this short-circuit
    # each "hand off to the freshly-spawned fiber" step costs a full
    # poll-timeout's worth of latency.
    def self.tick(poll_timeout_ms)
      # Reclaim trailing dead slots. Without this, the parallel
      # arrays grow once per accepted connection and never shrink --
      # a slow leak and per-tick iteration tax in a long-running
      # Scheduled server. Tail-only (stop at first alive) is
      # deliberate: it keeps every surviving slot's index stable,
      # so external captures of sched_current held across Fiber.yield
      # (e.g. pg.rb's PG::Pool @waiter_idxs) stay valid. Middle
      # dead slots aren't reclaimed until the tail catches up; for
      # FIFO request lifecycles that's the common case.
      i = Tep::APP.sched_fibers.length - 1
      while i >= 0 && !Tep::APP.sched_fibers[i].f.alive?
        Tep::APP.sched_fibers.delete_at(i)
        Tep::APP.sched_wake_at.delete_at(i)
        Tep::APP.sched_io_fd.delete_at(i)
        Tep::APP.sched_io_mode.delete_at(i)
        Tep::APP.sched_io_ready.delete_at(i)
        i -= 1
      end

      ms = poll_timeout_ms
      if Scheduler.any_time_ready
        ms = 0
      end
      Scheduler.poll_round(ms)

      now  = Sock.sphttp_now_us
      best = -1
      i = 0
      n = Tep::APP.sched_fibers.length
      while i < n
        if Tep::APP.sched_fibers[i].f.alive? && Tep::APP.sched_wake_at[i] <= now
          if best < 0 || Tep::APP.sched_wake_at[i] < Tep::APP.sched_wake_at[best]
            best = i
          end
        end
        i += 1
      end
      if best < 0
        return false
      end
      Tep::APP.sched_current = best
      Tep::APP.sched_wake_at[best] = -1
      Tep::APP.sched_fibers[best].f.resume
      Tep::APP.sched_current = -1
      true
    end

    # Build poll set from parked-on-I/O fibers, call poll(2), and
    # write observed-ready bits back into the parallel arrays.
    # `timeout_ms` is the poll() timeout (-1 = block forever,
    # 0 = non-blocking peek). Idempotent for an empty set.
    def self.poll_round(timeout_ms)
      Sock.sphttp_poll_reset
      slots = [-1] # slot index parallel to sched_fibers; -1 = not polled
      slots.clear
      added = 0
      i = 0
      n = Tep::APP.sched_fibers.length
      while i < n
        slot = -1
        if Tep::APP.sched_fibers[i].f.alive? &&
           Tep::APP.sched_io_fd[i] >= 0 &&
           Tep::APP.sched_io_ready[i] == 0
          slot = Sock.sphttp_poll_add(Tep::APP.sched_io_fd[i],
                                      Tep::APP.sched_io_mode[i])
          added += 1
        end
        slots.push(slot)
        i += 1
      end
      if added == 0
        return 0
      end
      Sock.sphttp_poll_run(timeout_ms)
      now = Sock.sphttp_now_us
      i = 0
      while i < n
        if slots[i] >= 0
          ready = Sock.sphttp_poll_ready(slots[i])
          if ready > 0
            Tep::APP.sched_io_ready[i] = ready
            Tep::APP.sched_wake_at[i]  = now
          end
        end
        i += 1
      end
      added
    end

    # Drain. Resumes everything ready until the schedulable set
    # is empty (every fiber finished or all are waiting for a
    # future wake_at / I/O). Returns the number of resumes performed.
    # Pure non-blocking; no poll() wait between passes.
    def self.run_until_empty
      n = 0
      while Scheduler.tick(0)
        n += 1
      end
      n
    end

    # Drain until `seconds` has elapsed OR every fiber's done.
    # Between empty passes, blocks in poll(2) (or sleep, if no
    # I/O waits) until the next wake-up.
    def self.run_for(seconds)
      # All bookkeeping is in microseconds (Sock.sphttp_now_us), matching
      # wake_at; poll() and sleep take ms / s respectively, so convert at
      # the call sites.
      deadline = Sock.sphttp_now_us + (seconds * 1000000).to_i
      while Sock.sphttp_now_us < deadline
        if !Scheduler.tick(0)
          # Nothing ready this pass. Compute the next deadline:
          # min(next_wake, overall_deadline). If any fiber is
          # parked on I/O, block in poll() until that or the
          # timer hits.
          next_at = Scheduler.next_wake
          gap = deadline - Sock.sphttp_now_us
          if next_at >= 0
            tgap = next_at - Sock.sphttp_now_us
            if tgap < gap
              gap = tgap
            end
          end
          if gap < 0
            gap = 0
          end
          if Scheduler.any_io_waiter
            # Park in poll for up to `gap` microseconds (poll wants ms).
            Scheduler.poll_round(gap / 1000)
          elsif next_at < 0
            return 0
          elsif gap > 0
            sleep(gap / 1000000.0)
          end
        end
      end
      0
    end

    def self.next_wake
      best = -1
      i = 0
      n = Tep::APP.sched_fibers.length
      while i < n
        if Tep::APP.sched_fibers[i].f.alive?
          if best < 0 || Tep::APP.sched_wake_at[i] < Tep::APP.sched_wake_at[best]
            best = i
          end
        end
        i += 1
      end
      if best < 0
        return -1
      end
      Tep::APP.sched_wake_at[best]
    end

    def self.any_io_waiter
      i = 0
      n = Tep::APP.sched_fibers.length
      while i < n
        if Tep::APP.sched_fibers[i].f.alive? &&
           Tep::APP.sched_io_fd[i] >= 0 &&
           Tep::APP.sched_io_ready[i] == 0
          return true
        end
        i += 1
      end
      false
    end

    # Is any alive fiber's wake_at already <= now? Used by tick() to
    # decide whether poll() can block: if anyone is time-due, the
    # poll timeout collapses to 0 (non-blocking peek) so we don't
    # waste wall time idling when there's runnable work.
    def self.any_time_ready
      now = Sock.sphttp_now_us
      i = 0
      n = Tep::APP.sched_fibers.length
      while i < n
        if Tep::APP.sched_fibers[i].f.alive? && Tep::APP.sched_wake_at[i] <= now
          return true
        end
        i += 1
      end
      false
    end

    # Called from within a fiber's body to suspend until at-or-
    # after `seconds` from now. Named `pause` rather than `sleep`
    # to keep the semantics distinct from `Kernel#sleep`: this is
    # a fiber-aware yield that returns the cooperative scheduler to
    # the dispatch loop, not an OS-level sleep. Outside a fiber it
    # falls through to bare `sleep(seconds)`.
    def self.pause(seconds)
      idx = Tep::APP.sched_current
      if idx < 0
        # Called from outside any fiber -- fall back to POSIX sleep.
        sleep(seconds)
        return 0
      end
      Tep::APP.sched_wake_at[idx] = Sock.sphttp_now_us + (seconds * 1000000).to_i
      Fiber.yield
      0
    end

    # Park the current fiber until `fd` is ready for the given
    # `mode` bits (1=READ, 2=WRITE, 3=both) OR `timeout_seconds`
    # elapses. Returns the observed-ready bits (0 on timeout).
    # When called from outside a fiber, falls back to a single
    # poll() call so the same code works at top level.
    def self.io_wait(fd, mode, timeout_seconds)
      idx = Tep::APP.sched_current
      if idx < 0
        # No fiber context -- single-shot poll inline.
        Sock.sphttp_poll_reset
        slot = Sock.sphttp_poll_add(fd, mode)
        Sock.sphttp_poll_run(timeout_seconds * 1000)
        return Sock.sphttp_poll_ready(slot)
      end
      Tep::APP.sched_io_fd[idx]    = fd
      Tep::APP.sched_io_mode[idx]  = mode
      Tep::APP.sched_io_ready[idx] = 0
      if timeout_seconds < 0
        # "Wait forever for I/O": -1 would mean "ready now" to the
        # tick picker, so use a far-future wake_at as the sentinel.
        Tep::APP.sched_wake_at[idx] = Sock.sphttp_now_us + 86400 * 1000000
      else
        Tep::APP.sched_wake_at[idx] = Sock.sphttp_now_us + (timeout_seconds * 1000000).to_i
      end
      Fiber.yield
      ready = Tep::APP.sched_io_ready[idx]
      Tep::APP.sched_io_fd[idx]    = -1
      Tep::APP.sched_io_mode[idx]  = 0
      Tep::APP.sched_io_ready[idx] = 0
      ready
    end

    # Reset the schedulable set. Useful between worker-loop
    # iterations or between tests.
    def self.clear
      while Tep::APP.sched_fibers.length > 0
        Tep::APP.sched_fibers.delete_at(0)
        Tep::APP.sched_wake_at.delete_at(0)
        Tep::APP.sched_io_fd.delete_at(0)
        Tep::APP.sched_io_mode.delete_at(0)
        Tep::APP.sched_io_ready.delete_at(0)
      end
      0
    end

    def self.alive_count
      n = 0
      i = 0
      total = Tep::APP.sched_fibers.length
      while i < total
        if Tep::APP.sched_fibers[i].f.alive?
          n += 1
        end
        i += 1
      end
      n
    end

    # True iff a Tep::Scheduler-managed fiber is currently executing.
    # Set by tick() right before f.resume and reset right after, so
    # this is the canonical "am I in cooperative context?" check for
    # callers that want to pick a blocking vs. fiber-yielding path
    # (e.g. Tep::Http -- see lib/tep/http.rb#send_req).
    def self.scheduled_context?
      Tep::APP.sched_current >= 0
    end
  end
end
