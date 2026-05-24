# Bounded pool of N opaque database-connection handles. Mirrors
# Rails AR's `ActiveRecord::ConnectionAdapters::ConnectionPool` shape
# (checkout / checkin / with_connection) and Tep's `PG::Pool` parking
# discipline (eager open at boot; free-list + waiter queue).
#
# Pool entries are opaque to the pool itself — the factory block passed
# at construction produces them. For SQLite that's a dbh (FFI :ptr under
# spinel; SQLite3::Database under the cruby gem); for future backends
# whatever `Db.open_connection` returns.
#
# Concurrency model:
#
#   - Under prefork (Tep::Server default): one Pool per worker process;
#     a single request holds one handle for its full duration. Size=1
#     is enough; size>1 only matters if a handler issues nested
#     checkouts (which Rails AR also discourages).
#
#   - Under Tep::Server::Scheduled (fiber-per-connection): one Pool
#     for the whole worker; concurrent fibers each take one handle.
#     Pool exhaustion parks the requesting fiber until a checkin
#     wakes it.
#
# Parking on exhaustion is deferred — the MVP raises on `checkout`
# when the free list is empty. Targets that adopt Scheduled mode wire
# in a per-target park primitive (Tep::Scheduler.pause for spinel,
# ConditionVariable for threaded targets) by overriding `wait_for_handle`.

module ActiveRecord
  module ConnectionAdapters
    class ConnectionPool
      attr_accessor :size, :free

      # Eagerly open `size` handles via the yielded block. If the
      # block raises mid-loop, partially-opened handles leak — the
      # caller is responsible for top-level rescue + retry. Uses
      # bare `yield` (not `&block` Proc binding) to match the
      # codebase convention in flash.rb / session.rb and avoid the
      # body-typer treating `&block` as a positional param.
      def initialize(size)
        @size = size
        @free = []
        i = 0
        while i < size
          @free.push(yield)
          i += 1
        end
      end

      # Acquire a handle. Raises if the pool is exhausted; replace
      # with `wait_for_handle` in scheduled contexts.
      def checkout
        if @free.length == 0
          wait_for_handle
        else
          @free.delete_at(0)
        end
      end

      # Return a handle to the free list.
      def checkin(handle)
        @free.push(handle)
      end

      # Pool-exhausted hook. MVP: raise. Override per-target to park.
      def wait_for_handle
        raise "ConnectionPool exhausted (size=" + @size.to_s + ")"
      end

      # Observability — useful for `pool.healthy?`-style assertions
      # and for the bench writeup to confirm the pool is sized right.
      def available_count
        @free.length
      end

      def in_use_count
        @size - @free.length
      end
    end
  end
end
