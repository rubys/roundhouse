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
# FORKED, ONE SOCKET. The parent binds; each child loads config.ru itself,
# so each opens its own SQLite handle (what config/puma.rb's
# before_worker_boot does for Puma) and the kernel spreads accept()s
# across them.

require "etc"
require "async"
require "async/container"
require "async/http/endpoint"
require "falcon/server"
require "protocol/rack"
require "rack/builder"

ROOT = __dir__
URL = "http://#{ENV.fetch("BIND", "0.0.0.0")}:#{ENV.fetch("PORT", "3000")}"
COUNT = Integer(ENV.fetch("WEB_CONCURRENCY", Etc.nprocessors.to_s))
# ROUNDHOUSE_CHECKPOINTER=0, for A/B measurement, leaves checkpoints to
# config.ru's per-worker thread, as under Puma.
CHECKPOINTER = ENV["ROUNDHOUSE_CHECKPOINTER"] != "0"

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
    container.run(count: COUNT, restart: true) do |instance|
      Sync do
        Dir.chdir(ROOT)
        app, = Rack::Builder.parse_file(File.join(ROOT, "config.ru"))
        # The WAL checkpoints run in the checkpointer process below, not
        # on a thread here that would freeze this event loop
        # (db.rb's checkpoint_in_own_process!).
        Db.checkpoint_in_own_process! if CHECKPOINTER && defined?(Db) && Db.respond_to?(:checkpoint_in_own_process!)
        # Jobs drain on this worker's event loop, not on a thread that
        # would take the GVL from it (active_job_cruby.rb's start_drainer).
        if defined?(ActiveJob) && ActiveJob.respond_to?(:start_drainer)
          ActiveJob.start_drainer { |work| Async(transient: true) { work.call } }
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
        Rack::Builder.parse_file(File.join(ROOT, "config.ru"))
        instance.ready!
        Db.run_checkpointer
        sleep
      end
    end
    $stdout.puts "falcon: #{COUNT} workers on #{URL}"
    $stdout.flush
  end
end

endpoint = Async::HTTP::Endpoint.parse(URL).with(reuse_address: true)
bound = Sync { endpoint.bound }
FalconController.new(endpoint, bound).run
