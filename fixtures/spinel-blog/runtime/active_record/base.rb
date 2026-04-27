require_relative "errors"
require_relative "validations"
require "time"

module ActiveRecord
  # Spinel has no `SingletonClassNode` handler in its codegen dispatch.
  # Replacing `class << self; attr_accessor :adapter; end` with two
  # explicit module-level methods backed by a module ivar gives the
  # same external API without the singleton-class node.
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

    def initialize
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

    # Subclasses override to mutate state from a row hash.
    def assign_from_row(_row)
      raise NotImplementedError, "#{self.class.name}#assign_from_row must be overridden"
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
      rows = ActiveRecord.adapter.where(table_name, conditions)
      rows.empty? ? nil : instantiate(rows.first)
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
      records = all
      records.each { |r| r.destroy }
      records
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

    def ==(other)
      other.is_a?(self.class) && @id != 0 && @id == other.id
    end

    def eql?(other)
      self == other
    end

    def hash
      [self.class, @id].hash
    end
  end
end
