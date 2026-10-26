# frozen_string_literal: true

# Cross-worker broadcasts for the CRuby overlay.
#
# `Cable::Registry` (cable.rb) knows only the sockets of the process it
# lives in. With one worker that is every socket; with several forked
# workers a message posted in one reaches only the people whose socket
# landed in the same one, and the rest never see it. Campfire's own
# deployment solves this with Redis behind Action Cable; this solves it
# with one more process on the same box, the shape falcon.rb already uses
# for the WAL checkpointer.
#
#   Hub.serve(path)       the hub process: a Unix socket that relays each
#                         message to the workers subscribed to its stream
#   Hub.connect(path)     a worker's transport for Broadcasts.set_transport:
#                         delivers to its own sockets at once, writes the
#                         message to the hub for the others, and hands what
#                         the hub relays to its own Registry
#
# ROUTED BY SUBSCRIPTION, as Redis is. A worker tells the hub when its
# Registry gains the first subscriber to a stream and when it loses the
# last, and the hub sends a message only to the workers holding its
# stream. Most broadcasts are for one person (campfire fans each post's
# unread notice out to every member's own stream), and that person's
# socket is in one worker.
#
# NO GAP AT SUBSCRIBE. The announcement is acknowledged before the
# Registry returns, and the Registry returns before the client is told
# its subscription is confirmed, so nothing published after a confirmed
# subscribe can miss it. Action Cable waits for Redis's own SUBSCRIBE
# confirmation for the same reason.
#
# The wire is tab-separated lines, the stream name in front so the hub
# routes without decoding the message:
#
#   worker -> hub   P <stream> <message JSON>    publish
#                   S <id> <stream>              subscribe (acknowledged)
#                   U <stream>                   unsubscribe
#   hub -> worker   P <stream> <message JSON>    a message for its sockets
#                   A <id>                       subscribe acknowledged
#
# A message is a rendered <turbo-stream> String or an
# `ActionCable.server.broadcast` Hash; both survive JSON unchanged, as far
# as `Registry.deliver` can tell (it re-encodes either with JSON.generate,
# and reads an internal disconnect's `type` under a String or Symbol key).
# Stream names are signed ids and channel names, never tabs or newlines.
#
# Each publisher's messages reach a worker in the order it sent them: one
# socket per worker, one relay thread per worker in the hub. Messages from
# different workers have no order between them, as with Redis.
require "socket"
require "json"

module Cable
  class Hub
    # How long a worker waits for the hub to start listening: the container
    # spawns the hub and the workers at the same time.
    CONNECT_TIMEOUT = 10
    # How long a subscribe waits for the hub's acknowledgement before
    # carrying on without it.
    ACK_TIMEOUT = 2

    def self.serve(path)
      File.unlink(path) if File.exist?(path)
      server = UNIXServer.new(path)
      routes = Hash.new { |h, stream| h[stream] = [] } # stream -> subscribed workers
      lock = Mutex.new
      loop do
        client = server.accept
        Thread.new(client) do |source|
          held = []
          source.each_line do |line|
            case line.getbyte(0)
            when 80 # P
              stream = line[2, line.index("\t", 2) - 2]
              targets = lock.synchronize { routes.key?(stream) ? routes[stream] - [source] : [] }
              targets.each do |target|
                target.write(line)
              rescue SystemCallError, IOError
                nil # that worker's own thread removes it
              end
            when 83 # S
              id, stream = line[2..].chomp.split("\t", 2)
              lock.synchronize do
                routes[stream] << source unless routes[stream].include?(source)
                held << stream
              end
              source.write("A\t#{id}\n")
            when 85 # U
              stream = line[2..].chomp
              lock.synchronize do
                routes[stream].delete(source)
                routes.delete(stream) if routes[stream].empty?
                held.delete(stream)
              end
            end
          end
        rescue SystemCallError, IOError
          nil
        ensure
          lock.synchronize do
            held.each do |stream|
              routes[stream].delete(source)
              routes.delete(stream) if routes[stream].empty?
            end
          end
          source.close unless source.closed?
        end
      end
    end

    def self.connect(path, local = Registry)
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + CONNECT_TIMEOUT
      begin
        hub = new(UNIXSocket.new(path), local)
      rescue Errno::ENOENT, Errno::ECONNREFUSED
        raise if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
        sleep 0.05
        retry
      end
      local.watch(hub) if local.respond_to?(:watch)
      hub
    end

    def initialize(socket, local)
      @socket = socket
      @local = local
      @lock = Mutex.new
      @acks = {}
      @acked = ConditionVariable.new
      @next_id = 0
      # A thread, not a fiber on the worker's event loop: it spends its life
      # blocked in a read, and hands each message to the Registry, which
      # posts it onto the cable reactor's own thread.
      @reader = Thread.new { relay_in }
    end

    # The Broadcasts transport. The local delivery comes first and does not
    # wait on the hub.
    def broadcast(stream, message)
      @local.broadcast(stream, message)
      write("P\t#{stream}\t#{JSON.generate(message)}\n")
    end

    # Registry watcher, called on the cable reactor's thread before the
    # subscription is confirmed to the client.
    def stream_added(stream)
      id = nil
      # Cable::Stats lives in cable.rb, which a hub can run without.
      asked = Stats.now if defined?(Stats) && Stats::ON
      @lock.synchronize do
        id = (@next_id += 1)
        @acks[id] = false
        @socket.write("S\t#{id}\t#{stream}\n")
        deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + ACK_TIMEOUT
        until @acks[id]
          left = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
          break if left <= 0
          @acked.wait(@lock, left)
        end
        warn "[cable hub] subscribe to #{stream} not acknowledged" unless @acks.delete(id)
      end
      Stats.record("hub", asked) if asked
      nil
    rescue SystemCallError, IOError => e
      warn "[cable hub] subscribe: #{e.class}: #{e.message}"
      nil
    end

    def stream_removed(stream)
      write("U\t#{stream}\n")
    end

    private
      def write(line)
        @lock.synchronize { @socket.write(line) }
        nil
      rescue SystemCallError, IOError => e
        warn "[cable hub] write: #{e.class}: #{e.message}"
        nil
      end

      def relay_in
        @socket.each_line do |line|
          if line.getbyte(0) == 65 # A
            id = Integer(line[2..])
            @lock.synchronize do
              @acks[id] = true if @acks.key?(id)
              @acked.broadcast
            end
          else
            tab = line.index("\t", 2)
            @local.broadcast(line[2, tab - 2], JSON.parse(line[tab + 1..]))
          end
        end
      rescue SystemCallError, IOError, JSON::ParserError => e
        warn "[cable hub] relay: #{e.class}: #{e.message}"
      end
  end
end
