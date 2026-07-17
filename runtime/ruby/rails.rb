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
  def self.env
    Env.new("development")
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

    def to_s
      @base
    end
  end

  def self.cache
    Cache.new
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

  # No-op cache: `fetch` always recomputes via its block.
  class Cache
    def fetch(key, opts = {})
      yield
    end

    def read(key)
      nil
    end

    def write(key, value)
      value
    end

    def delete(key)
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

  # App-config stand-in (see module note). Intentionally empty — the
  # app's real config methods aren't ingested, so they NameError rather
  # than being silently stubbed.
  class Application
  end
end
