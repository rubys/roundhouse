require_relative "errors"
require "time"

module ActiveRecord
  class << self
    attr_accessor :adapter
  end

  # An enum column assigned by LABEL. Rails' `enum :role, %i[member
  # administrator bot]` lets `user.role = "administrator"` store `1`;
  # the synthesized per-column writers cast to the column's slot type,
  # and `"administrator".to_i` is zero — which is `member`. campfire's
  # role change silently demoted the user rather than failing, which is
  # the worst shape a gap can take.
  #
  # `lower::enum_symbols` translates the labels an app writes DOWN
  # (`where(role: :bot)`); this is the half that only exists at
  # runtime, where the label came off the request.
  #
  # `labels`/`values` are parallel — the declaration's order — rather
  # than a Hash, because the emitters render a two-Array call with one
  # element type each and a `Hash[String, Integer]` literal argument is
  # a shape several of them place differently. Scanning three entries
  # costs nothing next to the write that follows.
  #
  # `text`, not an untyped value: the caller wraps the raw attrs read
  # in a `Cast` to String, which is the seam the strict targets already
  # use for every other attribute write — and it keeps this body
  # concretely typed, which the framework-runtime residual gate counts.
  #
  # A value that names no label falls through to `to_i`, which is what
  # an integer (or its string spelling) already meant — the same `to_i`
  # the per-column `Cast` did before this existed.
  def self.enum_int(text, labels, values)
    result = -1
    i = 0
    while i < labels.length
      result = values[i] if labels[i] == text
      i += 1
    end
    result = text.to_i if result == -1
    result
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
      # `0`, NOT `nil` — the UNSAVED SENTINEL, and a deliberate
      # divergence from Rails, which answers `Article.new.id == nil`
      # (measured). See docs/pipeline/runtime.md § Deliberate
      # divergences from Rails.
      #
      # It exists for the strict targets: a nullable primary key means
      # `Option<i64>` in Rust and `Int?` in Kotlin/Swift/C#, with an
      # unwrap at every foreign-key comparison, path helper and join.
      # The sentinel keeps ids plain machine integers across all
      # thirteen, and foreign keys use the same convention (the
      # synthesized `belongs_to` readers test `@creator_id == 0`).
      #
      # It is never the thing that answers "is this saved" — `@persisted`
      # below is. `form_with` picks its action from `persisted?`, and so
      # does `dom_id`; app code that reads `id` on an unsaved record is
      # where the divergence becomes visible.
      @id = 0
      @errors = []
      @id_previously_changed = false
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

    # The column this model treats as its identity. Unlike its
    # neighbours this one does NOT raise unoverridden: Rails' default is
    # `id` and almost every model takes it, so the lowering emits an
    # override only for the models that declared `self.primary_key =`.
    # Read by the upsert builder to name its conflict target.
    def self.primary_key
      "id"
    end

    # The temporal subset of `schema_columns`. Unlike its siblings this
    # one does NOT raise unoverridden: a model with no temporal column
    # legitimately has none, and the lowering emits the (possibly empty)
    # list for every schema-backed model. `_as_json_only` reads it to
    # decide which values need Rails' ISO8601 JSON form.
    def self.schema_time_columns
      []
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
    # on the small tables those models carry. Uses `_adapter_all` (not
    # `select_rows`) because the per-target `AdapterInterface` implements
    # `all`/`find`/`count` but not raw `select_rows` — and not bare
    # `all`, which the ruby-family connection.rb reopen overrides with a
    # lazy Relation (no `[-1]`); this wants the eager Array primitive.
    # Lowerer-emitted Level-3 models OVERRIDE this with a
    # `Db.prepare("... ORDER BY <pk> DESC LIMIT 1")` single-hydrate
    # (synth_adapter_last), so real apps get one row — lobsters' /u no
    # longer loads 10k users for its `User.last.id` cache key.
    def self._adapter_last
      records = _adapter_all
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

    # `record.has_attribute?(:token)` — does this model carry that
    # column? Answered from `schema_columns` rather than the
    # `attributes` hash, which is what Rails consults: the emitted
    # `attributes` omits `id`, so asking it would deny an attribute
    # every model has. The column list is also the honest answer here —
    # this runtime has no partial-select mode, so a loaded record
    # always carries every column.
    #
    # Monomorphic on Symbol ([[feedback_monomorphize_polymorphic_apis]]).
    # Rails also accepts a String; a String call site should coerce at
    # the lowering rather than widen this signature.
    def has_attribute?(name)
      # Bound to a local before the `include?`, matching
      # `fill_timestamps` below rather than chaining straight off
      # `self.class`. Not style: elixir2's receiver typing does not see
      # through the class-method send, so the direct chain lowers
      # `include?` as a struct-field access (`schema_columns().__struct__`)
      # and fails the warnings-as-errors gate. The local gives the
      # body-typer the Array it needs.
      cols = self.class.schema_columns
      cols.include?(name)
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
      # See `find_by` above for the `.to_h` rationale. The ruby-family
      # trees OVERRIDE this with a lazy-Relation version (connection.rb
      # reopen — dynamic call-sites chain off the fallback there);
      # this Array shape stays for the strict-target runtime
      # transpiles, which have no Relation class in their tables.
      ActiveRecord.adapter.where(table_name, conditions.to_h).map { |row| instantiate(row) }
    end

    def self.count
      _adapter_count
    end

    def self.exists?(id)
      _adapter_exists_by_id?(id)
    end

    # Rails delegates the Enumerable predicates from the class to `all`,
    # so `User.none?` asks whether the table has any row at all —
    # campfire's first-run check. Answered from COUNT rather than by
    # materializing: `none?`/`any?` on the class carry no conditions, so
    # there is nothing for the Relation to hold that the count doesn't.
    # The scoped forms (`User.where(…).none?`) go through Relation#none?
    # beside it.
    def self.none?
      count == 0
    end

    def self.any?
      count > 0
    end

    # Bulk DELETE without instantiating records or running callbacks —
    # ActiveRecord's `Model.delete_all` (used by seeds/tests for table
    # resets; `Relation#delete_all` covers the scoped form).
    def self.delete_all
      ActiveRecord.adapter.delete_all(table_name)
      nil
    end

    def self.destroy_all
      # `_adapter_all`, not `all`: the eager Array primitive, so the
      # return value is the destroyed records themselves (a lazy
      # Relation would re-query the now-empty table on a later read).
      records = _adapter_all
      records.each { |r| r.destroy() }
      records
    end

    # `Model.destroy_by(conditions)` — the UNSCOPED form of the method
    # `Relation#destroy_by` already answers. campfire's
    # `SessionsController#remove_push_subscription` writes
    # `Push::Subscription.destroy_by(endpoint:, user_id:)` on sign-out.
    #
    # Goes to the adapter rather than through `self.where`, for the
    # reason `destroy_all` reaches for `_adapter_all` next door: the
    # ruby-family trees OVERRIDE `where` with a lazy Relation (see the
    # note on `self.where`), and a Relation held across the destroys
    # would re-query rows the destroys have already removed. The
    # returned records are the destroyed ones, same as `destroy_all`.
    def self.destroy_by(conditions)
      records = ActiveRecord.adapter.where(table_name, conditions.to_h).map { |row| instantiate(row) }
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

      save_after_validation
    end

    # The post-validation half of `save`, extracted so Rails'
    # validation-skipping writes (`update_attribute`, in the
    # ruby-family connection.rb reopen) can enter here directly:
    # validations and their callbacks skipped, save callbacks run —
    # Rails' documented contract for those writes.
    def save_after_validation
      before_save
      was_new = new_record?
      if was_new
        before_create
        fill_timestamps(true)
        @id = _adapter_insert
        @persisted = true
        __track_saved_changes(was_new)
        after_create
        after_create_commit
      else
        before_update
        fill_timestamps(false)
        _adapter_update
        __track_saved_changes(was_new)
        after_update
        after_update_commit
      end
      after_save
      after_save_commit
      after_commit
      true
    end

    # ---- Saved-change tracking (ActiveModel::Dirty subset) ----------
    # Rails exposes what the last save changed via `saved_changes` /
    # `id_previously_changed?` / the per-attribute predicates the
    # lowerer synthesizes over `saved_changes` — moderation-log
    # callbacks branch on them (lobsters' Category/Tag#log_modifications).
    # These are STUBS: the attributes-hash diff lives in the
    # ruby-family-only connection.rb reopen — its heterogeneous
    # before/after hashes fought every strict transpile's type
    # inference (crystal unified the ivar across attributes overrides,
    # go typed the empty literals inconsistently), while the strict
    # lanes only need the surface to COMPILE for the synthesized
    # per-column predicates. Not-tracked is the honest strict-target
    # subset: `saved_changes` stays empty (the same empty-literal
    # shape as `attributes` above, which every target already types),
    # so every predicate answers false.
    def __track_saved_changes(was_new)
      @id_previously_changed = was_new
      nil
    end

    def saved_changes
      {}
    end

    # ONE leading underscore, like `_adapter_*` beside it: this is
    # called ACROSS the class boundary (a model's `from_row` reaching
    # Base's method), and Python mangles a `__`-prefixed name per class
    # — `Base.__note_hydrated` becomes `_Base__note_hydrated`, which the
    # subclass's call cannot see. The `__`-prefixed names in this file
    # (`__track_saved_changes`) are all called from within Base itself,
    # where the mangling is consistent.
    #
    # Called by the hydration factories (`from_row` / `from_stmt`) on a
    # record read from the DB: what it just received is its state as
    # SAVED, not a change. No-op here for the same reason
    # `saved_changes` is empty here — the snapshot lives in the
    # ruby-family connection.rb reopen, and the strict lanes carry the
    # not-tracked subset where every Dirty answer is already the honest
    # default.
    # EMPTY body, not `nil`, exactly like the lifecycle hooks below:
    # a `nil` tail on a void method emits as a bare `None;` in rust
    # ("cannot infer type of the type parameter T"). An empty body is
    # the shape every target already renders for a no-op.
    def _note_hydrated
    end

    # The argument-taking half of the ActiveModel::Dirty read surface,
    # for call sites that name the attribute at runtime rather than in
    # the method name (`saved_change_to_attribute?(:url)`,
    # `cols.map { |c| record.saved_change_to_attribute(c) }`). The
    # per-column `saved_change_to_<col>?` / `<col>_previously_changed?`
    # spellings are synthesized by the lowering instead, since a static
    # runtime cannot answer a name it only learns at emit time.
    #
    # Defined against `saved_changes` rather than the tracking ivar, so
    # they inherit whichever implementation the target has: the real
    # snapshot diff on the ruby-family trees (the connection.rb reopen)
    # and the empty-Hash subset on the strict lanes, where every Dirty
    # predicate already answers false by design.
    # Two shapes here are load-bearing, both learned from strict lanes
    # going red — neither is style, and neither is visible to
    # `compare ruby` or the unit tests:
    #
    # (1) `key?`, not `!saved_changes[name].nil?`. rust2 lowers an
    #     untyped map read to the total
    #     `.get().cloned().unwrap_or(Value::Null)`, and `.nil?` on that
    #     renders as `is_none()` — an Option method
    #     `serde_json::Value` does not have (E0599). The per-column
    #     synthesis CAN use the nil-test, because it runs against the
    #     model's own typed slot table rather than a json bag.
    #     Presence and non-nil are the same question here anyway:
    #     `__track_saved_changes` inserts a key only when the value
    #     actually changed, and always as a [prev, value] pair.
    #
    # (2) Bind the collection to a local before calling a collection
    #     method on it. elixir2's receiver typing does not see through
    #     a method-call receiver, so `saved_changes.key?(name)` lowers
    #     the `key?` as a struct-field access
    #     (`saved_changes(record).__struct__`) and fails the
    #     warnings-as-errors gate. `has_attribute?` above and
    #     `fill_timestamps` below take the same precaution.
    def saved_change_to_attribute?(name)
      changes = saved_changes
      changes.key?(name)
    end

    def saved_change_to_attribute(name)
      changes = saved_changes
      changes[name]
    end

    # The PREVIOUS value of an attribute the last save changed — the
    # value half of the Dirty pair, which the per-column
    # `<col>_previously_was` readers delegate to.
    #
    # NO `[0]` here, and that is the whole reason this method exists
    # instead of the readers indexing the diff themselves: the pair is
    # `[prev, value]` in a heterogeneous Hash, and indexing it renders
    # as an index on `interface{}` in go ("cannot index __prev"), on
    # `object?` in C#, and so on through every strict lane. The
    # indexing belongs in the ruby-family reopen, where the diff is
    # real; here the empty diff makes this nil, which is the honest
    # strict answer and matches every Dirty predicate answering false.
    #
    # It ENDS IN A READ rather than in a bare `nil`, exactly like
    # `saved_change_to_attribute` above
    # ([[project_shared_runtime_strict_return_shapes]]). A `nil` tail
    # gave swift no contextual type ("'nil' requires a contextual
    # type") and made rust emit an associated function rather than a
    # method, so `Article` could not find it at all.
    def attribute_previously_was(name)
      changes = saved_changes
      changes[name]
    end

    # `id` never appears in the subclass `attributes` hash, so the
    # created-vs-updated question is answered by the save path itself
    # rather than the snapshot diff.
    def id_previously_changed?
      @id_previously_changed
    end

    def save!
      raise RecordInvalid, self unless save
      self
    end

    # `record.touch` — write the current time to `updated_at` and
    # UPDATE, skipping validations and save callbacks. The catalog has
    # typed this as an instance DbWrite returning Bool since before it
    # existed here; this is the implementation catching up to the
    # declaration.
    #
    # NO-ARG ONLY, and that is a decision, not an omission. Rails'
    # `touch(:column)` names the extra column to stamp, which as a
    # shared method means `self[name] = …` — an index write through a
    # VARIABLE key. rust2 colors an index-write key for a Hash receiver
    # and for nothing else (`decide/str_color.rs` walks the key with
    # `ParentExpect::None` on the `LValue::Index` arm), so the owned
    # `String` lands in `set_index`'s `&str` slot and the app crate
    # fails to compile — E0308, and it takes every rust test with it.
    # The column form belongs at the CALL SITE, where the column is a
    # literal (`touch :connected_at` → `self.connected_at = …; touch`),
    # which is the same posture `insert_all` and `has_json` landed on:
    # inline what would otherwise need an untyped or dynamic parameter.
    # Until that lowering lands, `touch(:col)` raises ArgumentError,
    # which is the honest failure — campfire's `Membership#connected`
    # is the one corpus site.
    #
    # DIVERGENCE: Rails fires `after_touch` and the commit callbacks
    # here. This runtime has no `after_touch` hook, so a touch runs no
    # callbacks at all.
    def touch
      fill_timestamps(false)
      _adapter_update
      true
    end

    # `destroy!` — Rails raises `RecordNotDestroyed` when a
    # `before_destroy` callback throws `:abort`. This runtime has no
    # abort channel (`before_destroy` returns into the void and
    # `destroy` always completes), so the bang form is the same
    # operation. Written out rather than aliased for the strict
    # targets, and kept as its own method so the raise lands here when
    # an abort channel does exist.
    def destroy!
      destroy
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

    # Fired by the synthesized `initialize` tail and the hydration
    # factories (`from_row`/`from_stmt`) when a model declares it —
    # Rails: after new AND after find.
    def after_initialize;  end
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
