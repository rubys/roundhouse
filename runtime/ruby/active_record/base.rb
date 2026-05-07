require_relative "errors"
require_relative "validations"
require "time"

module ActiveRecord
  # Module-level adapter handle. `class << self; attr_accessor :adapter`
  # would be the idiomatic Ruby form — spinel itself supports that
  # pattern (matz/spinel#126), but rubocop_spinel 0.1.0 doesn't yet
  # allow it. Explicit accessor methods sidestep the tooling lag and
  # work identically.
  def self.adapter
    @adapter
  end

  def self.adapter=(value)
    @adapter = value
  end

  # Base class for all models. Designed to contain *zero* metaprogramming:
  # subclasses provide their own `attributes`, `[]`, `[]=`, `update`, and
  # `initialize`-from-attrs methods (typically by writing them out per
  # column). This Base class supplies the shared protocol — CRUD that
  # delegates to the adapter + validations + lifecycle hooks — without
  # any reflective access to ivars.
  class Base
    include Validations

    attr_accessor :id

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

    # Subclasses override to mutate state from a row hash. Error
    # message intentionally omits `self.class.name` — `.name`-style
    # reflection diverges across the 7 targets (`this.constructor.name`
    # vs `__MODULE__` vs `std::any::type_name`); the runtime stack
    # trace already identifies the receiver's class.
    def assign_from_row(_row)
      raise NotImplementedError, "assign_from_row must be overridden by subclass"
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
      ActiveRecord.adapter.all(table_name).map { |row| instantiate(row) }
    end

    def self.find(id)
      row = ActiveRecord.adapter.find(table_name, id)
      raise RecordNotFound, "Couldn't find #{name} with id=#{id}" if row.nil?
      instantiate(row)
    end

    def self.find_by(conditions)
      # `ActiveRecord.adapter.where` is typed `untyped` (the adapter
      # interface is target-specific), so the body-typer can't
      # narrow the return as `Array`. Avoid Array idioms that
      # require Ty::Array dispatch (`empty?`, `first`) — use
      # `length` and `[0]` which are JS-array-native and Ruby-
      # Array-native both. Same shape for every target.
      rows = ActiveRecord.adapter.where(table_name, conditions)
      return nil if rows.length == 0
      instantiate(rows[0])
    end

    def self.where(conditions)
      ActiveRecord.adapter.where(table_name, conditions).map { |row| instantiate(row) }
    end

    def self.count
      ActiveRecord.adapter.count(table_name)
    end

    def self.exists?(id)
      ActiveRecord.adapter.exists?(table_name, id)
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
    def self.create!(attrs = {})
      instance = new(attrs)
      raise RecordInvalid, instance unless instance.save
      instance
    end

    # `Article.last` — highest-id row, or nil when the table is
    # empty. Real-blog tests use it after a create-action redirect:
    # `assert_redirected_to article_url(Article.last)`. Implemented
    # via `all` rather than an adapter primitive so every adapter
    # gets it for free.
    def self.last
      records = all
      records.empty? ? nil : records[-1]
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
        fill_timestamps(creating: true)
        @id = ActiveRecord.adapter.insert(self.class.table_name, attributes)
        @persisted = true
        after_create
        after_create_commit
      else
        before_update
        fill_timestamps(creating: false)
        ActiveRecord.adapter.update(self.class.table_name, @id, attributes)
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
      ActiveRecord.adapter.delete(self.class.table_name, @id)
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
    # silently no-ops when the row no longer exists (a more
    # aggressive impl could raise RecordNotFound).
    def reload
      row = ActiveRecord.adapter.find(self.class.table_name, @id)
      return self if row.nil?
      assign_from_row(row)
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
    # the Rails ActiveRecord::Timestamp callback semantics (UTC ISO-8601).
    def fill_timestamps(creating:)
      cols = self.class.schema_columns
      now = Time.now.utc.iso8601
      self[:updated_at] = now if cols.include?(:updated_at)
      self[:created_at] = now if creating && cols.include?(:created_at) && self[:created_at].nil?
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
