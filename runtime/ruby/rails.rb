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

    # SECRET_KEY_BASE, parked the same way and for the same reason. The
    # app's own operator tooling is what produces it — campfire's
    # `script/admin/generate-secrets` prints `SECRET_KEY_BASE=<64 hex
    # bytes>` for the deployment to export, and Rails reads that same
    # variable — so keeping the contract means the emitted binary drops
    # into the same deployment rather than introducing config of its own.
    #
    # Unset reads as "". That is a loud failure, not a fallback: every
    # signature then verifies only against others made with the empty
    # key, so a cookie from a configured Rails will not validate and the
    # request reads as signed out. Generating one at boot would be worse
    # — it would look like it worked until the process restarted.
    attr_accessor :secret_key_base
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

    # `GlobalID.app` — the first segment of every `gid://<app>/<Model>/
    # <id>` URI this runtime mints. Rails derives it from the
    # application's railtie name (`campfire_application` minus the
    # suffix); ingest reads the module wrapping `class Application <
    # Rails::Application` in config/application.rb and synthesizes an
    # override here, exactly as it does for `session_cookie_key`.
    #
    # The default is deliberately generic: an app whose config/
    # application.rb ingest could not read still mints well-formed gids,
    # and since both ends of every gid round trip in a transpiled app are
    # this runtime, a name that merely stays CONSISTENT is enough to be
    # correct. It is only comparison against a real Rails process that
    # wants the app's own name, which is why ingest supplies it.
    def global_id_app
      "app"
    end

    # The key every signed message derives from — see the parked slot
    # on `Rails` itself, which the scaffold fills from the environment.
    def secret_key_base
      key = Rails.secret_key_base
      key.nil? ? "" : key
    end

    # Host for the absolute (`_url`) route helpers, which the view
    # lowerer grounds as `"http://#{Rails.application.domain}#{…_path}"`.
    #
    # A FRAMEWORK DEFAULT, exactly like `session_cookie_key` above, not a
    # stub for un-ingested config: Rails always has a host to build an
    # absolute URL from — it takes the REQUEST's. This reader existed
    # nowhere until now, because the only app in the corpus that reaches
    # the interpolation is lobsters, which happens to define
    # `Rails.application.domain` itself via `class << Rails.application`.
    # Every other app NameError'd at render on its first `_url` helper —
    # campfire's sign-in page, on `form_with url: session_url`.
    #
    # An app that DOES define it still wins: its `Rails::Application`
    # reopen is required after this file, same override path
    # `session_cookie_key` documents. So lobsters keeps its canonical
    # host and everyone else gets the request's.
    def domain
      req = ActionController::Current.request
      req.nil? ? "localhost" : req.host
    end

    # Rails' encrypted credentials store, as an EMPTY one.
    #
    # Not a stub standing in for work not done: the store lives in
    # `config/credentials.yml.enc` and only the master key opens it. That
    # key is deliberately not in the repo, so a transpiler cannot read
    # the values — and should not want to, since baking decrypted secrets
    # into an emitted tree is exactly the wrong place for them. The
    # secrets that DO reach the binary ride the environment (see the
    # parked `Rails.secret_key_base` slot above).
    #
    # Empty rather than absent because `dig` is how apps read it, and
    # Rails' own answer for an unconfigured key is nil: campfire writes
    # `ENV.fetch("VAPID_PUBLIC_KEY", Rails.application.credentials.dig(
    # :vapid, :public_key))`, which then falls back to the env var and,
    # unset, drops the meta tag — the same page Rails renders without
    # credentials configured.
    def credentials
      {}
    end

    # The zone every ActiveRecord temporal value is PRESENTED in. Same
    # framework-default shape as `session_cookie_key` above: Rails always
    # has one (`config.time_zone` defaults to "UTC"), ingest lifts an
    # app's `config.time_zone = "..."` into an override on the app's
    # reopen, and this stands in when the app declares none — so a
    # strict target can call it without a `respond_to?` guard.
    #
    # The value is a Rails zone NAME ("Central Time (US & Canada)"), not
    # an IANA identifier; `ActiveSupport::RAILS_TZ_TO_IANA` translates,
    # and main.rb pins ENV["TZ"] from the result before any render.
    def config_time_zone
      "UTC"
    end
  end
end

# GlobalID — the `gid://<app>/<Model>/<id>` identifier Rails mints for a
# record, and the shape a turbo stream name is built from.
#
# ONE SPELLING, which is the reason this exists at all rather than being
# interpolated at each call site. A stream name is written by the view
# (`turbo_stream_from @room, :messages`), written again by the model
# (`broadcast_append_to room, :messages`), and READ BACK by the channel
# that authorizes the subscription. Three places, and if any two
# disagree the message goes to a stream nobody is listening on, silently.
# The lowering already routes both write sides through one function; this
# is the same rule one level down, for the part that has to run at
# request time because it carries an id.
#
# Lives beside `Rails::Application#global_id_app` deliberately: the app
# name is half the URI. A fuller GlobalID — `Locator.locate`, which the
# subscribe side needs to turn a name back into a record — would earn its
# own file; minting alone does not.
module GlobalID
  # `record.to_gid_param`, spelled from parts the caller already knows.
  # globalid 1.3.0:
  #
  #   def to_gid_param(options = {}) = to_global_id(options).to_param
  #   def to_param = Base64.urlsafe_encode64(to_s, padding: false)
  #
  # The MODEL NAME is a parameter rather than read off the record, the
  # same rule `ActionText::SignedGlobalId.generate` and
  # `ActiveRecord::SignedId` state: the caller is a lowering with the
  # name already baked in, and reflection is what the strict targets do
  # not have.
  def self.param(model_name, id)
    Base64.urlsafe_encode64_nopad(uri(model_name, id))
  end

  # The unencoded URI, kept separate because it is the readable half —
  # a wrong app name or a wrong model name is obvious here and opaque
  # once base64 has been applied.
  def self.uri(model_name, id)
    "gid://" + Rails.application.global_id_app + "/" + model_name + "/" + id.to_s
  end
end
