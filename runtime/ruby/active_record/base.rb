require_relative "errors"
require "time"

module ActiveRecord
  class << self
    attr_accessor :adapter
  end

  # Base class for all models. Designed to contain *zero* metaprogramming:
  # subclasses provide their own `attributes`, `[]`, `[]=`, `update`, and
  # `initialize`-from-attrs methods (typically by writing them out per
  # column). This Base class supplies the shared protocol — CRUD that
  # delegates to the adapter + validations + lifecycle hooks — without
  # any reflective access to ivars.
  class Base
    attr_accessor :id

    # Error message accumulator populated by the lowerer-emitted
    # `validate` method (one `errors << "..."` per failed rule). Lives
    # on Base directly — Phase 2.5(a) inlined every `validates :x, …`
    # declaration, so the runtime `ActiveRecord::Validations` mixin no
    # longer ships. `errors` is reached via implicit-self Send in the
    # lowered IR; defining it here keeps that call resolvable.
    # `@errors` is initialized to `[]` in `initialize` — the prior
    # defensive `@errors = [] if @errors.nil?` lazy-init was redundant
    # AND compiler-hostile for typed targets (Rust struct types
    # `@errors` as `Vec<String>`, not `Option<Vec<String>>`; `.nil?`
    # on a Vec doesn't exist).
    def errors
      @errors
    end

    # `attrs = {}` keeps Base's constructor signature compatible
    # with subclasses that take attrs (`def initialize(attrs = {})`).
    # TS-side, this lets `new this(attrs)` in static `create` /
    # `create!` factories type-check against `typeof Base` whose
    # constructor signature is what TS sees at the dispatch site.
    # Body ignores attrs — subclass override is the place that
    # populates the column slots from the hash.
    def initialize(_attrs = {})
      @id = 0
      @errors = []
      @persisted = false
      @destroyed = false
    end

    # `Model.connection` / `Model.transaction` — the raw-SQL surface —
    # live in connection.rb's `class Base` reopen, NOT here: base.rb is
    # transpiled into every strict target's runtime (runtime_loader
    # tables), and that surface leans on constructs several emitters
    # don't lower yet (begin/rescue) plus a class those tables don't
    # ship. connection.rb is walked only into the ruby-family trees.

    # ---- Per-model overrides ----------------------------------------
    # Subclasses MUST override these. The base implementations exist as
    # contract markers; calling them on Base directly raises.

    def self.table_name
      raise NotImplementedError, "#{name}.table_name must be overridden"
    end

    def self.schema_columns
      raise NotImplementedError, "#{name}.schema_columns must be overridden"
    end

    def self.instantiate(_row)
      raise NotImplementedError, "#{name}.instantiate must be overridden"
    end

    # Eager-load hook: `Relation#to_a` calls this on its model with the
    # hydrated records and the recorded `includes(...)` specs. The Ruby
    # emit path synthesizes per-model overrides (batched IN-loads into
    # the `_preload_<assoc>` caches — see `apply_preload_lowering`);
    # models without one fall back to this no-op, which leaves the lazy
    # association readers doing the work (correct, just N+1).
    def self.preload_associations(_records, _specs)
    end

    # Per-model adapter primitives — public AR API delegates here.
    # Default implementations route through the legacy
    # `ActiveRecord.adapter.X` + `instantiate` path so subclasses that
    # haven't received the lowerer's Level-3 emit (and tests on `Base`
    # itself) keep working unchanged. The lowerer-emitted per-model
    # overrides go straight to the typed `Db.prepare` / `Db.column_*`
    # path — no Hash crossing the adapter boundary. Underscore-prefix
    # marks framework-internal; not part of the public AR API.

    def self._adapter_find_by_id(id)
      row = ActiveRecord.adapter.find(table_name, id)
      return nil if row.nil?
      instantiate(row)
    end

    def self._adapter_all
      ActiveRecord.adapter.all(table_name).map { |row| instantiate(row) }
    end

    # Default `_adapter_last` for hand-written subclasses / Base-level
    # tests — the original `all` + `[-1]`, correct everywhere and cheap
    # on the small tables those models carry. Uses `all` (not
    # `select_rows`) because the per-target `AdapterInterface` implements
    # `all`/`find`/`count` but not raw `select_rows`. Lowerer-emitted
    # Level-3 models OVERRIDE this with a `Db.prepare("... ORDER BY <pk>
    # DESC LIMIT 1")` single-hydrate (synth_adapter_last), so real apps
    # get one row — lobsters' /u no longer loads 10k users for its
    # `User.last.id` cache key.
    def self._adapter_last
      records = all
      records.empty? ? nil : records[-1]
    end

    # _adapter_insert / _adapter_update / _adapter_delete are
    # instance methods (not class methods) so Base#save / Base#destroy
    # call them via implicit-self dispatch — bypassing the
    # Abstract per-instance adapter primitives. Subclasses MUST
    # override — lowerer-emitted models (Article, Comment, …) get
    # `Db.exec` + `Db.last_insert_rowid` overrides per the Level-3
    # adapter-emit pipeline; hand-written subclasses opt into the
    # legacy `ActiveRecord.adapter.*` shim explicitly (see
    # `BaseTest::Item` in active_record/base_test.rb).
    #
    # Empty bodies are load-bearing for spinel-AOT: spinel's polymorphic
    # dispatch generates a class-id switch only when the base method
    # body is empty (matching the `after_create_commit`/etc. callback
    # pattern); a concrete base body causes monomorphic inlining to
    # base, which then no-ops because `ActiveRecord.adapter` isn't
    # wired under the Level-3 architecture.
    def _adapter_insert; end
    def _adapter_update; end
    def _adapter_delete; end

    def self._adapter_count
      ActiveRecord.adapter.count(table_name)
    end

    def self._adapter_exists_by_id?(id)
      ActiveRecord.adapter.exists?(table_name, id)
    end

    def self._adapter_truncate
      ActiveRecord.adapter.truncate(table_name)
    end

    # Refresh self's persisted columns from the DB (by @id), writing
    # back into self rather than constructing a new instance. Returns
    # self on success, nil when the row has been deleted. Empty Base
    # body for the same reason as `_adapter_insert`/etc. above —
    # spinel polymorphic dispatch needs the base method body empty so
    # the class-id switch fires; the lowerer-emitted per-model override
    # goes straight to `Db.prepare` / per-column ivar writes. Hand-
    # written subclasses (`BaseTest::Item`) provide their own override.
    def _adapter_reload; end

    # Subclasses override to return an attribute hash for adapter writes.
    def attributes
      {}
    end

    # Column-name indexer. Subclasses override with a per-column case
    # dispatch over the typed ivars (each model has a fixed set of
    # columns from the schema). The Base implementation raises so a
    # call on a record without a per-column override surfaces as an
    # error rather than silently returning nil.
    #
    # Defined here so abstract callers (FormBuilder.text_field's
    # `@model[field]`) type-check against `ActiveRecord::Base` at the
    # call site; Crystal needs the method to exist on the static type
    # for the call to compile.
    def [](_name)
      raise NotImplementedError, "[] must be overridden by subclass"
    end

    def []=(_name, _value)
      raise NotImplementedError, "[]= must be overridden by subclass"
    end

    # Subclasses MUST override to mutate state from a row hash. Empty
    # base body (rather than `raise NotImplementedError`) so spinel-AOT
    # generates a class-id switch at call sites — a concrete base body
    # causes monomorphic inlining to base, which then no-ops because
    # subclass overrides never get dispatched. See same pattern on
    # `_adapter_insert`/etc. above. The raise was a safety net for a
    # case that never fires in practice (every concrete model
    # overrides).
    def assign_from_row(_row); end

    # Per-model DOM prefix string ("article", "comment", ...). The
    # lowerer's `push_dom_prefix_method` synthesizes the actual constant-
    # returning body per concrete model so `dom_id(record)` resolves to
    # a known string at transpile time across every target (no
    # `record.class.name.downcase` reflection chain). The Base body
    # raises — calling `dom_prefix` on a bare ActiveRecord::Base would
    # indicate the per-model synthesizer didn't run for this class.
    def dom_prefix
      raise NotImplementedError, "dom_prefix must be overridden by subclass"
    end

    # ---- Persistence state ------------------------------------------

    def persisted?
      @persisted
    end

    def new_record?
      !@persisted
    end

    def destroyed?
      @destroyed
    end

    def mark_persisted!
      @persisted = true
      @destroyed = false
    end

    # ---- Class-level CRUD -------------------------------------------

    def self.all
      _adapter_all
    end

    def self.find(id)
      result = _adapter_find_by_id(id)
      raise RecordNotFound, "Couldn't find #{name} with id=#{id}" if result.nil?
      result
    end

    def self.find_by(conditions)
      # `ActiveRecord.adapter.where` is typed `untyped` (the adapter
      # interface is target-specific), so the body-typer can't
      # narrow the return as `Array`. Avoid Array idioms that
      # require Ty::Array dispatch (`empty?`, `first`) — use
      # `length` and `[0]` which are JS-array-native and Ruby-
      # Array-native both. Same shape for every target.
      #
      # `.to_h` on `conditions`: no-op on a Ruby Hash, NamedTuple→
      # Hash conversion under Crystal. Call sites that pass kwargs
      # (`Item.find_by(title: "B")`) lift to NamedTuple in Crystal,
      # but the adapter's `where` slot is typed `Hash(Symbol, _)`.
      rows = ActiveRecord.adapter.where(table_name, conditions.to_h)
      return nil if rows.length == 0
      instantiate(rows[0])
    end

    # `find_by!` — `find_by` that raises `RecordNotFound` (→ 404) instead
    # of returning nil on no match.
    def self.find_by!(conditions)
      result = find_by(conditions)
      raise RecordNotFound, "Couldn't find #{name}" if result.nil?
      result
    end

    def self.where(conditions)
      # See `find_by` above for the `.to_h` rationale.
      ActiveRecord.adapter.where(table_name, conditions.to_h).map { |row| instantiate(row) }
    end

    def self.count
      _adapter_count
    end

    def self.exists?(id)
      _adapter_exists_by_id?(id)
    end

    # Bulk DELETE without instantiating records or running callbacks —
    # ActiveRecord's `Model.delete_all` (used by seeds/tests for table
    # resets; `Relation#delete_all` covers the scoped form).
    def self.delete_all
      ActiveRecord.adapter.delete_all(table_name)
      nil
    end

    def self.destroy_all
      records = all()
      records.each { |r| r.destroy() }
      records
    end

    # `Article.create(title: "...", body: "...")` — convenience that
    # constructs and saves in one call. Mirrors Rails' `create`. The
    # Hash-shaped constructor signature accepts the kwargs-as-hash
    # the seed scripts use (`Article.create(title: ..., body: ...)`).
    def self.create(attrs = {})
      instance = new(attrs)
      instance.save
      instance
    end

    # `Article.create!(...)` — bang variant: raises RecordInvalid
    # when validation fails instead of returning the unsaved
    # instance. Used by seeds and tests that expect creation to
    # succeed unconditionally; failure is a fatal error rather
    # than a flow-control branch.
    # Rails' block form (`create! do |kv| ... end`) is grounded at emit
    # — `apply_create_block_inline` expands the call site into
    # new/block-body/save — so the runtime signature stays blockless on
    # every target (a `yield` here forced a block param onto all twelve
    # transpiled runtimes and broke their 1-arg callers).
    def self.create!(attrs = {})
      instance = new(attrs)
      raise RecordInvalid, instance unless instance.save
      instance
    end

    # `Article.last` — highest-id row, or nil when the table is empty.
    # Real-blog tests use it after a create-action redirect:
    # `assert_redirected_to article_url(Article.last)`. Delegates to
    # `_adapter_last` (ORDER BY <pk> DESC LIMIT 1, one row) rather than
    # `all` + `[-1]`, which materialized the whole table — lobsters' /u
    # cache key is `User.last.id` over 10k+ users, run per request.
    def self.last
      _adapter_last
    end

    # ---- Instance lifecycle ------------------------------------------

    def save
      before_validation
      ok = valid?
      after_validation
      return false unless ok

      before_save
      if new_record?
        before_create
        fill_timestamps(true)
        @id = _adapter_insert
        @persisted = true
        after_create
        after_create_commit
      else
        before_update
        fill_timestamps(false)
        _adapter_update
        after_update
        after_update_commit
      end
      after_save
      after_save_commit
      after_commit
      true
    end

    def save!
      raise RecordInvalid, self unless save
      self
    end

    def destroy
      return self unless persisted?
      before_destroy
      _adapter_delete
      @persisted = false
      @destroyed = true
      after_destroy
      after_destroy_commit
      after_commit
      self
    end

    # Re-fetch the row by id and reassign all column slots. Mirrors
    # Rails' `record.reload` — used after a controller action that
    # updates the row, to refresh the in-memory copy. Returns self;
    # silently no-ops when the row no longer exists.
    #
    # NOTE: still uses the legacy `ActiveRecord.adapter.find` Hash-
    # returning path — the typed `_adapter_find_by_id` returns a
    # whole instance, but `assign_from_row` (the per-model contract)
    # expects a row Hash. Migrating reload to typed-instance copy
    # requires either an `assign_from_instance` lowering or an
    # `[]=`-based field copy that subclasses override (today's Item
    # subclass in base_test doesn't override `[]`/`[]=`). Deferred.
    def reload
      # Delegates to the lowerer-emitted (or Base default) per-model
      # `_adapter_reload` instance primitive, which re-reads the row
      # by `@id` and writes column values back into self (preserving
      # identity). Implicit-self dispatch (no `self.class` chain) so
      # async profiles emit `await _adapter_reload(this)` cleanly
      # rather than awaiting the receiver Send. Returns self on
      # success, self unchanged when the row has been deleted.
      _adapter_reload
      self
    end

    # ---- Lifecycle hooks (no-ops; subclasses override) --------------

    def before_validation; end
    def after_validation;  end
    def before_save;       end
    def after_save;        end
    def before_create;     end
    def after_create;      end
    def before_update;     end
    def after_update;      end
    def before_destroy;    end
    def after_destroy;     end
    def after_commit;      end
    def after_create_commit;  end
    def after_update_commit;  end
    def after_destroy_commit; end
    def after_save_commit;    end
    def after_touch;          end

    # Subclasses define their own `validate` if they need any.
    def validate; end

    # Fills `created_at` (on insert) and `updated_at` (always) when the
    # subclass declares those columns in `schema_columns`. Uses the
    # subclass's `[]=` to assign — no `instance_variable_set`. Mirrors
    # the Rails ActiveRecord::Timestamp callback semantics.
    #
    # `ActiveSupport.db_now` stamps Rails' exact storage form —
    # "YYYY-MM-DD HH:MM:SS.ffffff", UTC, space separator, zero-padded
    # 6-digit fractional seconds, no zone marker — so a column's TEXT
    # values stay homogeneous (and lexicographically ordered) when a
    # roundhouse-emitted app writes into a Rails-created database.
    # Each target maps the intrinsic to its native clock+format helper;
    # CRuby/JRuby resolve it in active_support_time_parsing.rb.
    #
    # `created_at` is stamped on every insert, unconditionally — we do
    # NOT read the column back and skip when already set. The earlier
    # `self[:created_at].nil?` guard meant well (don't clobber a value
    # the caller pre-assigned), but it was the source of a cross-target
    # bug: targets that type string columns non-nullable (TS/Crystal/
    # Rust/Swift) initialize a fresh record's `created_at` to `""`, not
    # nil, so the guard never fired and the column shipped empty — which
    # collapsed `ORDER BY created_at DESC` to insertion (rowid) order.
    # The blank-check is also hard to express portably: the generic `[]`
    # accessor returns a different type per target (String, serde_json::
    # Value, Any?), so a literal `== ""` won't type-check everywhere.
    # An unconditional stamp sidesteps all of that and matches how
    # `updated_at` above is already handled.
    #
    # Positional `creating` (was kwarg `creating:`). Kwargs in Ruby
    # call sites lower to a Hash arg; rust2 emit doesn't yet unflatten
    # the Hash back to a positional bool, so the call becomes
    # `fill_timestamps({"creating" => true})` and fails to match the
    # method's `bool` param. Positional sidesteps that — TS/Crystal
    # accept either shape, Rust gets the simpler one.
    def fill_timestamps(creating)
      cols = self.class.schema_columns
      now = ActiveSupport.db_now
      self[:updated_at] = now if cols.include?(:updated_at)
      self[:created_at] = now if creating && cols.include?(:created_at)
    end

    def valid?
      @errors = []
      validate
      @errors.empty?
    end

    # ---- Equality ---------------------------------------------------
    #
    # Ruby's `==` / `eql?` / `hash` equality protocol is intentionally
    # not defined here. The protocol is Ruby-specific (used by Hash
    # keys and Set membership) and has no cross-target analog: TS
    # `Map`/`Set` use `===` reference equality, Rust uses `Eq`/`Hash`
    # derives, etc. Per-target runtimes that need value equality
    # implement it on the appropriate target shape (e.g.
    # `juntos.ts`'s ApplicationRecord exposes `equals(other)` if
    # callers need it). Adding the methods to base.rb produced
    # broken emit (`[Klass, @id].hash` has no JS equivalent) without
    # any caller benefit.
  end
end
