module ActionDispatch
  # Per-app flash store. Typed-field replacement for HashWithIndifferent-
  # Access in `@flash`. The field set (`notice`, `alert`) is the closed
  # set of keys the lowerer recognizes (see view_to_library/extra_params.rs
  # `is_flash_name`). Apps that introduce new flash keys grow this struct
  # in lockstep with the lowerer.
  #
  # Typed targets (TS/Crystal/Rust) get dot-syntax field access AND the
  # HWIA-shape shim methods (`fetch`, `key?`, `length`, `[]`, `[]=`, …)
  # so framework tests and controller bodies that reach in via the
  # Symbol-keyed bracket form keep compiling. The shim's `[]/[]=` route
  # through a closed `key.to_s` case so the typer doesn't fan the
  # receiver across an untyped Hash channel.
  class Flash
    attr_accessor :notice, :alert
    # Read-only accessors so typed targets infer `String?` for the
    # snapshot ivars via the same accessor-nilability path as
    # notice/alert (bare ivars get a narrower inference).
    attr_reader :notice_was, :alert_was

    # `other` is an optional plain Hash carrying flash state across
    # requests (the server persists flash between redirect_to and the
    # follow-on GET). Nil at first-request boundary; populated on
    # follow-on requests. Keys read as Strings — the server's
    # persistent store is String-keyed.
    #
    # Flash lifecycle (the Rails "show exactly once" rule, owned here so
    # every target's server is just a storage adapter): the constructor
    # snapshots the loaded values as `@notice_was` / `@alert_was`. A key
    # is carried to the next request by `to_persisted` only if the action
    # CHANGED it from that snapshot (i.e. set a new flash this request);
    # a key merely loaded-and-displayed is unchanged, so it drops out and
    # the notice shows exactly once. (The snapshot comparison, rather than
    # hooking `[]=`, is deliberate: typed targets reach the fields
    # natively — `flash["notice"] = x` is a direct field write in TS, not
    # a `[]=` method call — so freshness can't live in `[]=`.)
    def initialize(other = nil)
      @notice = nil
      @alert  = nil
      # Snapshot of the carried-in values; `to_persisted` diffs against
      # these. Assigned alongside @notice/@alert (same nil-or-String
      # shape) so target type inference reads them as `String?`.
      @notice_was = nil
      @alert_was  = nil
      return if other.nil?
      # Iterate rather than index: the persisted store is partial ({} /
      # notice-only / alert-only), and strict targets (Crystal) raise
      # KeyError on a missing Hash key. `each` only visits present keys
      # and is a proven cross-target idiom (see `merge`).
      other.each do |k, v|
        if k == "notice"
          @notice = v
          @notice_was = v
        elsif k == "alert"
          @alert = v
          @alert_was = v
        end
      end
    end

    # HWIA-shape `[key]` accessor — accepts Symbol or String key. Routes
    # through a closed case so each branch's return ty is the
    # corresponding field's ty (`String?`), not a fanned-out untyped
    # channel.
    def [](key)
      k = key.to_s
      return @notice if k == "notice"
      return @alert  if k == "alert"
      nil
    end

    def []=(key, value)
      k = key.to_s
      if k == "notice"
        @notice = value
      elsif k == "alert"
        @alert = value
      end
      value
    end

    def fetch(key, default = nil)
      v = self[key]
      return v if !v.nil?
      default
    end

    def key?(key)
      v = self[key]
      !v.nil?
    end

    def has_key?(key)
      key?(key)
    end

    def include?(key)
      key?(key)
    end

    def delete(key)
      k = key.to_s
      if k == "notice"
        v = @notice
        @notice = nil
        return v
      end
      if k == "alert"
        v = @alert
        @alert = nil
        return v
      end
      nil
    end

    # Count of populated fields. Both `length` and `size` inline the
    # same logic so Crystal's auto-rewrite of `length` → `size` (a
    # Hash idiom shim) doesn't recurse the methods through each other
    # when one calls the other.
    def length
      n = 0
      n += 1 if !@notice.nil?
      n += 1 if !@alert.nil?
      n
    end

    def size
      n = 0
      n += 1 if !@notice.nil?
      n += 1 if !@alert.nil?
      n
    end

    def empty?
      @notice.nil? && @alert.nil?
    end

    def keys
      result = []
      result.push("notice") if !@notice.nil?
      result.push("alert")  if !@alert.nil?
      result
    end

    def values
      result = []
      result.push(@notice) if !@notice.nil?
      result.push(@alert)  if !@alert.nil?
      result
    end

    # Yields `(String, String)` pairs for populated fields only — matches
    # HWIA's String-keyed yield shape so view extra-param plumbing
    # doesn't fork on receiver type.
    def each
      yield "notice", @notice if !@notice.nil?
      yield "alert",  @alert  if !@alert.nil?
      self
    end

    # Plain String-keyed Hash of populated fields. Server-side flash
    # persistence reads `.to_h()` between requests; the persistent
    # store is `Hash[String, String]`.
    def to_h
      result = {}
      result["notice"] = @notice if !@notice.nil?
      result["alert"]  = @alert  if !@alert.nil?
      result
    end

    # The entries to carry to the NEXT request — only those the action
    # CHANGED from what it loaded (i.e. set a new flash this request).
    # Entries loaded from the previous request and merely displayed are
    # unchanged, so they drop out and a notice shows exactly once. Every
    # target's server persists this between requests (in-memory var,
    # cookie, …) and reloads it via `new`, which is what makes the sweep
    # a property of Flash rather than per-server logic.
    def to_persisted
      result = {}
      # Bind to locals so strict targets narrow `String? → String` across
      # the compound guard (ivars don't narrow reliably in Crystal).
      n = @notice
      result["notice"] = n if !n.nil? && n != @notice_was
      a = @alert
      result["alert"]  = a if !a.nil? && a != @alert_was
      result
    end

    # `merge(other)` returns a new Flash with `other`'s entries layered
    # on top. `other` may be a Hash or another Flash — either responds
    # to `each` with `(k, v)` pairs.
    def merge(other)
      result = Flash.new
      result.notice = @notice
      result.alert  = @alert
      other.each do |k, v|
        result[k] = v
      end
      result
    end
  end
end
