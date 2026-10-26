# frozen_string_literal: true

# Falcon, serving config.ru behind ThrusterCache::Front.
#
#   bundle exec ruby falcon.rb        # PORT, BIND, WEB_CONCURRENCY
#
# Needs the falcon gem, which the emitted Gemfile does not name; the
# bench's ruby-falcon lane adds it (scripts/bench-campfire).
#
# WHY NOT `falcon serve -c config.ru`. That wraps the app in Falcon's Rack
# adapter and nothing else can sit in front of it, so every response pays
# for a Rack env and a header conversion: ~14 us a request on the bench
# box, against ~3 us for a ThrusterCache hit itself. Here the cache's
# Protocol::HTTP front answers hits first and only the rest reach the
# adapter, the shape the hand-ported campfire on Falcon runs
# (tobi/campfire-once-ruby-ractor, lib/campfire/server.rb). `falcon serve`
# also adds Protocol::HTTP::ContentEncoding, which the app does not need:
# config.ru's GzipCache already compresses HTML, as Rack::Deflater does in
# campfire's own config.ru.
#
# FORKED, ONE SOCKET, PRELOADED. The parent loads config.ru, binds, and
# forks; the kernel spreads accept()s across the children. Loading in the
# parent is what config/puma.rb's preload_app! does, and for the same
# reason: the app's code, constants and boot-time heap are then shared
# copy-on-write instead of loaded once per child. Measured 2026-10-22 on
# campfire (11 workers, rubix2): 699 -> 131 MB PSS at boot, 1,092 -> 822
# MB serving /rooms/1. `Process.warmup` before the fork (GC, compaction,
# old-generation promotion) took that to 725. Each child still opens its
# own SQLite handles: Db.adopt_after_fork replaces the parent's pool on
# the first request in a new pid, as it does under Puma.
#
# ROUNDHOUSE_PRELOAD=0 restores the per-child load, for A/B measurement.
#
# ONE CABLE, MANY WORKERS. Each child's Cable::Registry holds only its own
# sockets, so with more than one worker a broadcast would reach only the
# people connected to the worker that sent it. A hub process relays every
# broadcast to the other workers (runtime/cable_hub.rb), as Redis does for
# campfire's own deployment. ROUNDHOUSE_CABLE_HUB=0 leaves each worker on
# its own Registry, for A/B measurement.

require "etc"
require "async"
require "async/container"
require "async/http/endpoint"
require "falcon/server"
require "protocol/rack"
require "rack/builder"
require "tmpdir"
require_relative "runtime/cable_hub"

ROOT = __dir__
URL = "http://#{ENV.fetch("BIND", "0.0.0.0")}:#{ENV.fetch("PORT", "3000")}"
COUNT = Integer(ENV.fetch("WEB_CONCURRENCY", Etc.nprocessors.to_s))
# ROUNDHOUSE_CHECKPOINTER=0, for A/B measurement, leaves checkpoints to
# config.ru's per-worker thread, as under Puma.
CHECKPOINTER = ENV["ROUNDHOUSE_CHECKPOINTER"] != "0"
PRELOAD = ENV["ROUNDHOUSE_PRELOAD"] != "0"
CABLE_HUB = COUNT > 1 && ENV["ROUNDHOUSE_CABLE_HUB"] != "0"
CABLE_HUB_PATH = File.join(Dir.tmpdir, "roundhouse-cable-hub-#{Process.pid}.sock")

# async-container's controller owns the children: INT/TERM stop them
# (gracefully first) before the parent exits, so the port is free when
# the process is gone.
class FalconController < Async::Container::Controller
  def initialize(endpoint, bound)
    @endpoint = endpoint
    @bound = bound
    super(container_class: Async::Container::Forked)
  end

  def setup(container)
    endpoint, bound = @endpoint, @bound
    # The hub first: workers wait for it to listen (Cable::Hub.connect).
    if CABLE_HUB
      container.spawn(name: "cable-hub", restart: true) do |instance|
        instance.ready!
        Cable::Hub.serve(CABLE_HUB_PATH)
      end
    end
    container.run(count: COUNT, restart: true) do |instance|
      Sync do
        Dir.chdir(ROOT)
        app, = PRELOAD ? APP : Rack::Builder.parse_file(File.join(ROOT, "config.ru"))
        # The WAL checkpoints run in the checkpointer process below, not
        # on a thread here that would freeze this event loop
        # (db.rb's checkpoint_in_own_process!).
        Db.checkpoint_in_own_process! if CHECKPOINTER && defined?(Db) && Db.respond_to?(:checkpoint_in_own_process!)
        # Jobs drain on this worker's event loop, not on a thread that
        # would take the GVL from it (active_job_cruby.rb's start_drainer).
        if defined?(ActiveJob) && ActiveJob.respond_to?(:start_drainer)
          ActiveJob.start_drainer { |work| Async(transient: true) { work.call } }
        end
        if CABLE_HUB && defined?(Broadcasts) && defined?(Cable::Registry)
          Broadcasts.set_transport(Cable::Hub.connect(CABLE_HUB_PATH))
        end
        if defined?(Cable::Stats) && Cable::Stats::ON
          Cable::Stats.ensure_reporting
          # How late the event loop runs a timer: what blocks it, every
          # request on this worker waits out.
          Async(transient: true) do
            loop do
              due = Cable::Stats.now + 0.01
              sleep 0.01
              Cable::Stats.record("loop_lag", due)
              Cable::Stats.note_gc
            end
          end
        end
        front = ThrusterCache::Front.new(Protocol::Rack::Adapter.new(app))
        server = Falcon::Server.new(front, bound, protocol: endpoint.protocol, scheme: endpoint.scheme)
        instance.ready!
        server.run.wait
      end
    end
    # One more process, serving nothing: the WAL checkpointer.
    if CHECKPOINTER
      container.spawn(name: "checkpointer", restart: true) do |instance|
        Dir.chdir(ROOT)
        Rack::Builder.parse_file(File.join(ROOT, "config.ru")) unless PRELOAD
        instance.ready!
        Db.run_checkpointer
        sleep
      end
    end
    $stdout.puts "falcon: #{COUNT} workers on #{URL}#{" (cable hub #{CABLE_HUB_PATH})" if CABLE_HUB}"
    $stdout.flush
  end
end

if PRELOAD
  Dir.chdir(ROOT)
  APP, = Rack::Builder.parse_file(File.join(ROOT, "config.ru"))
  Process.warmup
end
endpoint = Async::HTTP::Endpoint.parse(URL).with(reuse_address: true)
bound = Sync { endpoint.bound }
FalconController.new(endpoint, bound).run
