# frozen_string_literal: true
# Pushing ActiveJob payloads to Redis the way Sidekiq 8.1's ActiveJob
# adapter does, for an app on `config.active_job.queue_adapter =
# :sidekiq`. A Ruby Sidekiq process, in a Rails app with the same job
# classes and database, then runs the jobs.
#
# THE BYTES ARE SIDEKIQ'S. The ActiveJob payload (`Payload.build_at`)
# goes inside Sidekiq's job envelope, with the keys in the order
# Sidekiq writes them:
#
#   {"retry":true,"queue":"default","wrapped":"InspectCarJob",
#    "args":[<payload>],"class":"Sidekiq::ActiveJob::Wrapper",
#    "jid":"<24 hex>","created_at":<ms>,"enqueued_at":<ms>}
#
# An immediate job is `SADD queues <queue>` plus `LPUSH queue:<queue>`
# in one round trip; a scheduled one is `ZADD schedule <epoch> <json>`,
# with no `enqueued_at`. `tests/sidekiq_push.rs` holds this to what real
# Sidekiq 8.1.7 sends (`tests/sidekiq_push/sidekiq.json`).
#
# NOTHING HERE NAMES `Redis`. The connection lives in
# `runtime/sidekiq_redis.rb`, which a tree on `:sidekiq` gets with a
# real client (spinel-redis on spinel, redis-rb on ruby and jruby) and
# every other tree gets as a raising façade (`project::spin_shape` and
# `ruby_family_runtime_files` do the swap). Nothing calls this module
# unless the lowering wrote a `JobRegistry.push_<job>` call, which it
# does only under `:sidekiq`.
#
# The Redis URL is the app's own `Sidekiq.configure_client` `url:`
# (`JobRegistry.sidekiq_redis_url`) when it gives one. Only without it
# does the push fall back to Sidekiq's `ENV[ENV["REDIS_PROVIDER"]]`,
# then `REDIS_URL`, then `redis://localhost:6379/0`. A `password:` from
# the same block overrides one in the URL, as redis-rb's option does.
# Both are read at the first push in each process. Errors reach the
# caller, as in Rails: a down Redis makes `perform_later` raise.
module SidekiqPush
  DEFAULT_URL = "redis://localhost:6379/0"
  URL = [""]
  URL_PID = [0]

  # The test seam: while recording, pushes are written to RECORDED as
  # text instead of going to Redis, so a test needs no server and no
  # client. Same shape as `SecureRandom.stub_uuid`.
  RECORDING = [false]
  RECORDED = [""]

  # One push held until COMMIT (`enqueue_after_commit`).
  class HeldPush
    def initialize(job_class, queue, at, aj_json)
      @job_class = job_class
      @queue = queue
      @at = at
      @aj_json = aj_json
    end

    def job_class
      @job_class
    end

    def queue
      @queue
    end

    def at
      @at
    end

    def aj_json
      @aj_json
    end
  end

  # One job, given its ActiveJob payload. `at` is the scheduled epoch in
  # seconds, 0.0 when the job isn't scheduled. A time that has already
  # passed is pushed at once, as Sidekiq's `Setter#at` does.
  def self.enqueue(job_class, queue, at, aj_json)
    head = "{\"retry\":true" +
      ",\"queue\":" + ActiveJob::Arguments.str(queue) +
      ",\"wrapped\":" + ActiveJob::Arguments.str(job_class) +
      ",\"args\":[" + aj_json + "]" +
      ",\"class\":\"Sidekiq::ActiveJob::Wrapper\"" +
      ",\"jid\":" + ActiveJob::Arguments.str(SecureRandom.hex(12)) +
      ",\"created_at\":" + now_ms.to_s
    if at > Time.now.to_f
      schedule(at, head + "}")
    else
      push(queue, head + ",\"enqueued_at\":" + now_ms.to_s + "}")
    end
    nil
  end

  # `enqueue`, held while a transaction is open and pushed at its COMMIT
  # (a job that sets `enqueue_after_transaction_commit = true`).
  def self.enqueue_after_commit(job_class, queue, at, aj_json)
    if ActiveJob.in_transaction
      held.push(HeldPush.new(job_class, queue, at, aj_json))
    else
      enqueue(job_class, queue, at, aj_json)
    end
    nil
  end

  def self.held
    h = Thread.current[:sidekiq_held_until_commit]
    if h.nil?
      h = [HeldPush.new("", "", 0.0, "")]
      h.clear
      Thread.current[:sidekiq_held_until_commit] = h
    end
    h
  end

  # At COMMIT: the hold is emptied first, so a push that fails can't
  # leak held jobs into this thread's next transaction. Every held job
  # is tried, and the first error is raised after the rest.
  def self.flush_held
    h = held
    if h.length == 0
      return nil
    end
    jobs = h.dup
    h.clear
    failed = false
    first = StandardError.new("")
    jobs.each do |j|
      begin
        # `queue` is an attr-reader String; push only reads it (SADD /
        # LPUSH key material). Concatenating "" is a new string so
        # Spinel's share analysis does not hand the shared handle into
        # those calls.
        enqueue(j.job_class, j.queue + "", j.at, j.aj_json)
      rescue StandardError => e
        if !failed
          failed = true
          first = e
        end
      end
    end
    if failed
      raise first
    end
    nil
  end

  def self.drop_held
    held.clear
    nil
  end

  def self.push(queue, json)
    if RECORDING[0]
      RECORDED.push("sadd queues " + queue)
      RECORDED.push("lpush queue:" + queue + " " + json)
      return nil
    end
    SidekiqRedis.push(url, queue, json)
  end

  def self.schedule(at, json)
    if RECORDING[0]
      RECORDED.push("zadd schedule " + at.to_s + " " + json)
      return nil
    end
    SidekiqRedis.schedule(url, at, json)
  end

  # Sidekiq's clock for created_at and enqueued_at: integer epoch
  # milliseconds.
  def self.now_ms
    Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
  end

  # The app's `configure_client` URL, else Sidekiq's
  # `determine_redis_provider`, else its default.
  def self.url
    if URL_PID[0] != Process.pid
      URL[0] = resolve_url
      URL_PID[0] = Process.pid
    end
    URL[0]
  end

  def self.resolve_url
    configured = JobRegistry.sidekiq_redis_url
    if configured != ""
      return configured
    end
    provider = ENV["REDIS_PROVIDER"].to_s
    if provider.include?(":")
      raise ArgumentError.new("REDIS_PROVIDER should be set to the name of the variable which contains the Redis URL, not a URL itself")
    end
    named = provider == "" ? "" : ENV[provider].to_s
    if named != ""
      return named
    end
    env = ENV["REDIS_URL"].to_s
    env == "" ? DEFAULT_URL : env
  end

  # The app's `configure_client` `password:`, `""` for none (the URL's
  # own password, if any, then stands).
  def self.password
    JobRegistry.sidekiq_redis_password
  end

  # A `configure_client` setting read from the environment, for the
  # registry's generated `sidekiq_redis_url` and `_password`:
  # `ENV["NAME"]`, `""` when unset.
  def self.setting(name)
    ENV[name].to_s
  end

  # `ENV.fetch("NAME", "default")`. An empty variable counts as unset.
  def self.setting_or(name, default)
    value = ENV[name].to_s
    value == "" ? default : value
  end

  # `ENV.fetch("NAME")`, which raises when the variable is unset, as the
  # app's initializer would at boot.
  def self.required_setting(name)
    value = ENV[name].to_s
    if value == ""
      raise KeyError.new("key not found: \"" + name + "\" (Sidekiq.configure_client's Redis setting)")
    end
    value
  end

  # The URL with its password replaced by REDACTED, for the boot line,
  # as Sidekiq's own `scrub` does.
  def self.redacted_url(value)
    scheme = value.index("://")
    if scheme.nil?
      return value
    end
    start = scheme + 3
    tail = value[start, value.length - start]
    at = tail.rindex("@")
    if at.nil?
      return value
    end
    userinfo = tail[0, at]
    colon = userinfo.index(":")
    shown = colon.nil? ? "REDACTED" : userinfo[0, colon] + ":REDACTED"
    value[0, start] + shown + tail[at, tail.length - at]
  end

  # The boot line a server on `:sidekiq` logs. It doesn't connect:
  # Sidekiq's client connects at the first push too.
  def self.announce
    warn "[job] queue_adapter :sidekiq: jobs are pushed to " + redacted_url(url) + "; run them with a Sidekiq process"
    nil
  end

  def self.start_recording
    RECORDING[0] = true
    RECORDED.clear
    nil
  end

  def self.stop_recording
    RECORDING[0] = false
    nil
  end

  def self.recorded
    RECORDED.dup
  end
end
