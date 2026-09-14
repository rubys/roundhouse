# concurrent-ruby's thread pools and barrier, over spinel's own threads.
#
# THE RUBY FAMILY NEVER LOADS THIS FILE. `project::ruby_runtime_files`
# swaps it for a bare `require "concurrent"`: over there the real gem
# is in the bundle (sentry-ruby already pulls it in), and a second
# `Concurrent::ThreadPoolExecutor` beside it would reopen the gem's
# class with a different `initialize`. Same arrangement as ipaddr and
# zlib — the tree that has the real one uses it, and this port exists
# for the lane that has none.
#
# WHY A PORT AND NOT A FACADE. campfire's `WebPush::Pool` (lib/) builds
# a `Concurrent::ThreadPoolExecutor` and a `FixedThreadPool` at boot and
# posts every push-notification delivery to them; a facade that raised
# would take the message-create request down with it (the job runs
# in-process). spinel has real M:N threads with `Mutex`, `Queue` and
# `ConditionVariable` (docs/thread.md), which is everything a pool is
# made of, so the honest answer is to run the tasks.
#
# ONLY THE SURFACE THE APP AND ITS SUITE REACH IS PORTED — `post`,
# `shutdown`, `kill`, `wait_for_termination`, `completed_task_count`
# and the size readers — plus `CyclicBarrier#wait` for the
# race-condition test. The gem's tuning knobs (`idletime`, the
# fallback policies other than `:abort`, `min_threads` pre-spawn) are
# accepted where a caller passes them and do nothing: a worker here is
# a green thread parked on a Queue, which costs nothing to keep.
#
# Semantics kept from the gem, because the suite observes them:
# * A task posted when no worker is idle and the pool is under
#   `max_threads` spawns a worker; otherwise it queues; a queue at
#   `max_queue` (when > 0) raises `RejectedExecutionError`, as does a
#   post after `shutdown`. The `:abort` policy, the gem's default.
# * `completed_task_count` counts tasks that RAN, raised or not. The
#   pool never lets a task's exception escape the worker.
# * `shutdown` lets queued tasks drain; `kill` drops them and stops the
#   workers; `wait_for_termination(t)` answers whether every worker
#   exited within `t` seconds.
#
# Two shapes are avoided on purpose, each a filed spinel gap:
# * Liveness is a counter the worker decrements on exit, not
#   `Thread#alive?` — through an Array element that method raises
#   (matz/spinel#4463).
# * No block posted from inside this file captures an iteration
#   parameter — a `&blk` stored for later shares the cell
#   (matz/spinel#4462). Callers' blocks are theirs to write.
module Concurrent
  class RejectedExecutionError < StandardError
  end

  class ThreadPoolExecutor
    def initialize(min_threads: 0, max_threads: 8, max_queue: 0, idletime: 60, fallback_policy: :abort)
      @max_threads = max_threads
      @max_queue = max_queue
      @queue = Queue.new
      @lock = Mutex.new
      @workers = []
      @live = 0
      @idle = 0
      @completed = 0
      @running = true
    end

    def max_length
      @max_threads
    end

    def length
      n = 0
      @lock.synchronize { n = @live }
      n
    end

    def queue_length
      @queue.size
    end

    def completed_task_count
      n = 0
      @lock.synchronize { n = @completed }
      n
    end

    def running?
      r = false
      @lock.synchronize { r = @running }
      r
    end

    def shutdown?
      !running?
    end

    # The gem spawns when no worker is READY — parked on the queue
    # waiting for a task. A worker counts itself idle only around its
    # own `pop`, so a burst of posts ahead of the first pick-up grows
    # the pool toward `max_threads` rather than queueing on one thread.
    def post(&task)
      spawn = false
      @lock.synchronize do
        raise RejectedExecutionError, "pool is shut down" unless @running
        if @idle == 0 && @live < @max_threads
          spawn = true
          @live += 1
        elsif @max_queue > 0 && @queue.size >= @max_queue
          raise RejectedExecutionError, "queue is full"
        end
        # Value position: the `if` above has no `else`, and with a
        # raising arm spinel types the join by the first arm alone
        # (matz/spinel#4464). The block's value is not read.
        nil
      end
      spawn_worker if spawn
      @queue.push(task)
      true
    end

    def <<(task)
      post(&task)
      self
    end

    # Closing the queue is the shutdown signal: a worker's `pop` answers
    # nil once the queue is closed AND drained, so queued tasks still
    # run and the worker exits after the last one.
    def shutdown
      @lock.synchronize { @running = false }
      @queue.close
      nil
    end

    def kill
      ws = []
      @lock.synchronize do
        @running = false
        ws = @workers.dup
      end
      @queue.close
      @queue.clear
      ws.each { |w| w.kill }
      @lock.synchronize { @live = 0 }
      nil
    end

    # Seconds as a Float rather than a `Time?` deadline: a nil-guarded
    # `Time >= deadline` on the nullable raised at run time on spinel.
    def wait_for_termination(timeout = nil)
      deadline = timeout.nil? ? -1.0 : Time.now.to_f + timeout.to_f
      loop do
        return true if length == 0
        return false if deadline >= 0.0 && Time.now.to_f >= deadline
        sleep 0.005
      end
    end

    private

    def spawn_worker
      t = Thread.new do
        loop do
          @lock.synchronize { @idle += 1 }
          task = @queue.pop
          @lock.synchronize { @idle -= 1 }
          break if task.nil?
          begin
            task.call
          rescue Exception => e
            nil
          end
          @lock.synchronize { @completed += 1 }
        end
        @lock.synchronize do
          @live -= 1
          @workers.delete(Thread.current)
        end
      end
      @lock.synchronize { @workers.push(t) }
      nil
    end
  end

  class FixedThreadPool < ThreadPoolExecutor
    def initialize(num_threads, max_queue: 0)
      super(min_threads: num_threads, max_threads: num_threads, max_queue: max_queue)
    end
  end

  # `parties` threads call `wait`; each blocks until all have, then all
  # proceed together. Reusable: the generation counter is what lets a
  # late arrival tell "everyone left" from "nobody came yet".
  class CyclicBarrier
    def initialize(parties)
      @parties = parties
      @waiting = 0
      @generation = 0
      @lock = Mutex.new
      @cv = ConditionVariable.new
    end

    def parties
      @parties
    end

    def number_waiting
      n = 0
      @lock.synchronize { n = @waiting }
      n
    end

    def wait(timeout = nil)
      @lock.synchronize do
        gen = @generation
        @waiting += 1
        if @waiting == @parties
          @waiting = 0
          @generation += 1
          @cv.broadcast
        else
          while gen == @generation
            @cv.wait(@lock)
          end
        end
      end
      true
    end
  end
end
