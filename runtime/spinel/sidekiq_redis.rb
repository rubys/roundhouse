# frozen_string_literal: true
# The Redis connection `SidekiqPush` pushes through — the façade every
# tree gets by default. A tree on `queue_adapter :sidekiq` with a
# payload job gets `runtime/spinel/sidekiq/sidekiq_redis.rb` here
# instead, with a real client (`project::spin_shape` on spinel,
# `ruby_family_runtime_files` on ruby and jruby), the same swap point
# shape as `bcrypt_facade.rb`.
#
# Nothing reaches these: the lowering writes `JobRegistry.push_<job>`
# calls only under `:sidekiq`, and that is exactly when the real file is
# swapped in. They raise rather than drop a job.
module SidekiqRedis
  def self.push(url, queue, json)
    raise "queue_adapter :sidekiq: this tree has no Redis client, so the job for queue " + queue + " can't be pushed"
  end

  def self.schedule(url, at, json)
    raise "queue_adapter :sidekiq: this tree has no Redis client, so a scheduled job can't be pushed"
  end
end
