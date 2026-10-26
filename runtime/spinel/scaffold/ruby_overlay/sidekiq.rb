# frozen_string_literal: true
# The `sidekiq` role on ruby and jruby: run jobs from Redis instead of
# serving HTTP, as `./app sidekiq` does on spinel.
#
#   ruby sidekiq.rb [-c N] [-q queue[,weight]]... [-t seconds]
#
# Only a tree on queue_adapter :sidekiq ships this file. See
# runtime/sidekiq_worker.rb for the options and the shutdown sequence.
require_relative "main"

SidekiqWorker.prepare_pool(ARGV)
Main.configure_default_adapter!
Db.checkpoint_in_background!
ActiveJob.register_drain
SidekiqWorker.cli(ARGV)
