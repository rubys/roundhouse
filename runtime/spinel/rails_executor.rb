# `Rails.application.executor.wrap { … }` — Rails' way of running work
# on a thread the framework did not start: the block gets the
# per-unit-of-work context a request has, which for this runtime means
# a database connection lease. campfire's web-push pool wraps its
# invalid-subscription handler in one, because that handler runs on the
# pool's own thread and destroys a row.
#
# RUBY-FAMILY ONLY, required by both boots (spinel's scaffold and the
# CRuby overlay): the lease is `Db.with_connection`, which the strict
# targets do not spell the same way, and the shared `Rails::Application`
# in runtime/ruby/rails.rb is transpiled to every one of them.
#
# RE-ENTRANT, as Rails' is. A thread already inside a lease — a request
# thread, or the job drain, which takes one around the whole pass —
# runs the block as it is; taking a second lease would rebind the
# thread's connection and, on release, unbind the outer one under a
# request still using it.
module Rails
  class Application
    def executor
      Executor.new
    end
  end

  class Executor
    def wrap
      if Db.in_lease?
        yield
      else
        Db.with_connection { yield }
      end
    end
  end
end
