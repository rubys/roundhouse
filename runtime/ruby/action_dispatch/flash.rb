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

    # `other` is an optional plain Hash carrying flash state across
    # requests (the server persists flash between redirect_to and the
    # follow-on GET). Nil at first-request boundary; populated on
    # follow-on requests. Keys read as Strings — the server's
    # persistent store is String-keyed.
    def initialize(other = nil)
      @notice = nil
      @alert  = nil
      return if other.nil?
      v = other["notice"]
      @notice = v if !v.nil?
      v = other["alert"]
      @alert = v if !v.nil?
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
