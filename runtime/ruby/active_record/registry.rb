# Cross-model registry. Bridges the import-cycle gap in strict-typed
# targets (Go, Rust, future Kotlin/Swift) where `Article has_many
# :comments` + `Comment belongs_to :article` can't both be expressed
# as direct type references — the package/crate boundary forces a
# DAG. See GitHub issue #20 for the full design.
#
# Lazy targets (TS, Crystal, Spinel, Ruby) don't need the registry to
# break cycles — their language-level lazy loaders (Zeitwerk for Ruby,
# ES module cycles for TS, autoload for Crystal) resolve forward
# references without help. The registry transpiles to those targets as
# a no-op shim so the framework runtime stays uniform across all 7
# targets; emitters that don't need it simply never call `lookup`.
#
# Strict targets: the `model_associations` lowerer marks each cycle
# edge with `Resolution::Registry`. Emitters that see that marker
# register their model on load (`Registry.register("Comment", …)`)
# and resolve cross-cycle reads through `Registry.lookup(...)` rather
# than a direct type reference.
#
# The registry value is `untyped` here — concrete shape varies per
# target. Ruby/Spinel/Crystal store the class itself; Rust/Go store a
# factory closure or trait object. Callers cast at the lookup site.

module ActiveRecord
  module Registry
    @entries = {}

    # Register `klass` under `name`. Idempotent — re-registering
    # overwrites the previous binding. Typically called once per
    # model class at load time.
    def self.register(name, klass)
      @entries[name] = klass
    end

    # Return the registered value for `name`, or `nil` if no entry
    # exists. Missing-on-lookup is silent; targets that want a hard
    # failure should wrap the call site (e.g. `Registry.lookup(n) ||
    # raise "missing model #{n}"`).
    def self.lookup(name)
      @entries[name]
    end

    # True iff `name` has been registered. Cheap predicate for
    # emit-time decisions that want a fallback path when the model
    # isn't present.
    def self.registered?(name)
      @entries.has_key?(name)
    end

    # Drop every entry. Test-support only; production code never calls
    # this. Kept in the contract so test fixtures can isolate runs
    # without leaking state across cases.
    def self.clear!
      @entries = {}
    end
  end
end
