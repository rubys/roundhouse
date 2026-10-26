# frozen_string_literal: true
# The `sidekiq` role for a tree that isn't on `queue_adapter :sidekiq`:
# there is nothing in Redis for it to run, so `./app sidekiq` says so
# and exits 1. A tree on `:sidekiq` with a payload job gets
# `runtime/spinel/sidekiq/sidekiq_worker.rb` here instead, the same swap
# as `sidekiq_redis.rb`.
module SidekiqWorker
  def self.prepare_pool(argv)
    nil
  end

  def self.cli(argv)
    $stderr.print "sidekiq: this app doesn't use queue_adapter :sidekiq, so there are no jobs in Redis to run\n"
    exit(1)
  end
end
