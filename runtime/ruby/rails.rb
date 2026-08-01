# frozen_string_literal: true

# Minimal, statically-resolvable `Rails` global for the roundhouse
# runtime — `Rails.env` / `Rails.cache` / `Rails.logger` plus an (empty)
# `Rails.application` config stand-in.
#
# Deliberately metaprogramming-free: every method is explicit so the
# strict-target (spinel AOT) compile and the runtime typing bar both hold
# — no `method_missing`, no built-in subclassing. `Rails.application`'s
# real methods are app-specific config roundhouse doesn't ingest yet, so
# they surface as honest gaps rather than being dynamically stubbed.
#
# `Rails.cache` is a no-op store (every `fetch` recomputes via its block);
# correct, just not actually caching, which is adequate until a real cache
# backend is wired.
module Rails
  class << self
    # RAILS_ENV, parked by the scaffold main.rb at boot (the runtime
    # typing gate doesn't model `ENV[]`, so the read lives in the
    # scaffold). Same global-slot idiom as `ActiveRecord.adapter`.
    attr_accessor :env_name
  end

  # Rails-faithful: the parked RAILS_ENV wins, development is the
  # default when unset (serving/bench harnesses pass
  # RAILS_ENV=production; lobsters gates dev-only filters on
  # `Rails.env.development?`).
  def self.env
    name = self.env_name
    Env.new(name.nil? || name.empty? ? "development" : name)
  end

  # `Rails.root` — the app root. Rails hands back a Pathname; the
  # corpus both interpolates it (`"#{Rails.root}/x"` — AppPath#to_s
  # keeps that byte-identical at ".") and chains `.join("tmp/…")`
  # (lobsters' blocklist job), so the AppPath stand-in serves both.
  # The emitted app serves from its root, hence ".".
  def self.root
    AppPath.new(".")
  end

  # `Rails.public_path` — Rails returns a Pathname; the corpus chains
  # `.join("avatars/").to_s` (lobsters' avatar cache dir). AppPath is
  # the minimal typed stand-in: join concatenates with a single
  # separator, to_s reads the accumulated path. Rooted at "public"
  # relative to the emitted app root (matching Rails.root's "."
  # grounding above).
  def self.public_path
    AppPath.new("public")
  end

  # Plain value object, no Pathname subclassing (the runtime stays
  # statically resolvable).
  class AppPath
    def initialize(base)
      @base = base
    end

    def join(part)
      AppPath.new(@base + "/" + part)
    end

    # `Rails.root + "storage/x"` — Pathname#+ is a path join, not string
    # concatenation, so it is `join` under another name.
    def +(part)
      AppPath.new(@base + "/" + part)
    end

    def to_s
      @base
    end

    # The implicit path conversion `File.read` / `IO.read` / `File.open`
    # look for. Rails.root is a Pathname there, and Pathname answers
    # `to_path` (NOT `to_str` — it deliberately does not pose as a
    # String). Without this, `File.read(Rails.root.join(…))` raises
    # `TypeError: no implicit conversion of Rails::AppPath into String`
    # — which is what SearchParser hit through
    # `FetchIanaTldsJob.tlds`, once its rules were emitted at all.
    def to_path
      @base
    end
  end

  # One store per process. A fresh `Cache.new` per call would make every
  # `fetch_str` a miss, which is the no-op this used to be.
  def self.cache
    @cache_store ||= Cache.new
  end

  def self.logger
    Logger.new
  end

  def self.application
    Application.new
  end

  # `Rails.env.production?` etc. — a plain object answering the known
  # environment predicates (no `method_missing`, no `String` subclass).
  class Env
    def initialize(name)
      @name = name
    end

    def development?
      @name == "development"
    end

    def production?
      @name == "production"
    end

    def test?
      @name == "test"
    end

    def staging?
      @name == "staging"
    end
  end

  # `Rails.cache`, in two halves.
  #
  # `fetch` stays the recompute-every-call no-op it has always been. Its
  # value is whatever the block returns — an Integer count, a Relation, a
  # rendered fragment — and one store holding all of those needs the
  # heterogeneous box this runtime deliberately doesn't have.
  #
  # `fetch_str` is the typed half: String keys, String values, expiry at
  # second granularity. `src/lower/rails_cache.rs` routes the fetch sites
  # whose block provably yields a String — the rendered fragments — here,
  # and those are where the cost is. Lobsters caches its users tree for
  # 24h; recomputing that ~292KB fragment on every request is 15 of the
  # 114 visits in the benchmark sequence and most of the spinel lane's
  # iteration time.
  #
  # Process-local and unsynchronized, matching the serving shape (one
  # request at a time per process). ActiveSupport::Cache::MemoryStore
  # holds a Mutex because Puma runs threads; a threaded server here needs
  # the same before this is safe.
  class Cache
    def initialize
      @entries = {}
      @expires_at = {}
    end

    def fetch(key, opts = {})
      yield
    end

    # `ttl` is a whole number of seconds; 0 never expires. Expiry is
    # checked lazily on read, as MemoryStore does.
    def fetch_str(key, ttl)
      k = key.to_s
      if @entries.key?(k)
        due = @expires_at[k]
        return @entries[k] if due == 0 || due > Time.now.to_i
        @entries.delete(k)
        @expires_at.delete(k)
      end
      value = yield
      @entries[k] = value
      @expires_at[k] = ttl > 0 ? Time.now.to_i + ttl : 0
      value
    end

    def read(key)
      nil
    end

    def write(key, value)
      value
    end

    # Invalidation reaches the typed store — lobsters deletes
    # `user:<id>:unread_replies` on write paths, and a stale fragment
    # surviving its explicit delete would be a behaviour change, not a
    # missing optimization.
    def delete(key)
      k = key.to_s
      @entries.delete(k)
      @expires_at.delete(k)
      nil
    end

    def exist?(key)
      false
    end
  end

  # No-op logger — the request path doesn't depend on log output.
  class Logger
    def info(message); end
    def error(message); end
    def warn(message); end
    def debug(message); end
    def fatal(message); end
  end

  # App-config stand-in (see module note). Otherwise empty — the app's
  # real config methods aren't ingested, so they NameError rather than
  # being silently stubbed.
  class Application
    # Name of the cookie the session travels in. This is a FRAMEWORK
    # DEFAULT, not a stub for un-ingested app config: Rails always has a
    # session cookie name (it derives one from the app name when the app
    # declares none), and the dispatch has to know it before any app code
    # runs. `config/initializers/session_store.rb`'s
    # `config.session_store :cookie_store, key: "..."` is lifted at
    # ingest into a `session_cookie_key` def on the app's
    # `Rails::Application` reopen, which is required right after this
    # file and so overrides this default.
    #
    # Both readers must agree or login breaks: the dispatch round-trips
    # the cookie under this name, and lobsters' `remove_unknown_cookies`
    # deletes every cookie whose key isn't the configured one — a
    # disagreement clears the session on every request.
    def session_cookie_key
      "_session"
    end
  end
end
