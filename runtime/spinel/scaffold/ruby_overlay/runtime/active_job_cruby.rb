# CRuby serving: `perform_later` puts the work down and returns.
#
# The shared runtime has the queue (`ActiveJob.enqueue` / `drain`) but
# only uses it once something registers a drain; until then a job runs
# inline, because a job queued where nothing drains it is a job dropped.
# The spinel server registers one (`Main.job_loop`); the Puma server did
# not, so campfire's POST ran web-push delivery (`Room::PushMessageJob`)
# inside the request: 13% of it on rubix3. Rails hands that job to its
# queue adapter and answers the POST.
#
# `config.ru` calls `drain_in_background!`. Each process that then
# enqueues starts its own drain thread on first use (Puma forks workers
# after config.ru, and threads do not survive a fork), keyed by pid like
# the WAL checkpointer. The thread blocks on a Thread::Queue rather than
# polling, and takes its own Db lease per pass, as `Main.job_loop` does.
# The queue itself is thread_state's: a shared Array under a lock.
# Nothing is retried and nothing survives a restart: the ledgered limit
# of the in-process queue (docs/pipeline/runtime.md).
module ActiveJob
  @drain_pid = nil
  @drain_mutex = Mutex.new
  @wake = nil
  @serving = false

  def self.drain_in_background!
    register_drain
    @serving = true
    nil
  end

  # PERFORMED is the test harness's log of what ran; a server never
  # reads it, and appending a name per job would grow it for as long as
  # the process lives.
  class << self
    alias_method :record_performed_for_tests, :record_performed
    alias_method :enqueue_locked, :enqueue
  end

  def self.record_performed(job_name)
    return nil if @serving
    record_performed_for_tests(job_name)
  end

  def self.enqueue(work)
    start_drainer if @drain_pid != Process.pid
    enqueue_locked(work)
    @wake << true
    nil
  end

  def self.start_drainer
    @drain_mutex.synchronize do
      return if @drain_pid == Process.pid
      wake = Thread::Queue.new
      @wake = wake
      @drain_pid = Process.pid
      Thread.new { drain_loop(wake) }
    end
  end

  def self.drain_loop(wake)
    loop do
      wake.pop
      wake.clear
      begin
        Db.with_connection { drain } while pending_count > 0
      rescue StandardError => e
        warn "[job] drain failed: " + e.message
      end
    end
  end
end
