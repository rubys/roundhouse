module ActiveRecord
  class Base
    include Validations
    include Broadcasts

    # ── Per-class schema metadata ───────────────────────────────────
    #
    # Each subclass tracks its schema column names via an override of
    # attr_accessor. Calling `attr_accessor :id, :title, ...` in a
    # subclass does two things:
    # 1. Generates standard @ivar-backed getter/setter pairs (Ruby's
    #    built-in behavior).
    # 2. Appends the names to the subclass's column list, which Base
    #    consults when serializing to/from the adapter.
    #
    # This makes each schema column a typed ivar (@title: String, etc.)
    # rather than a polymorphic Hash lookup — a representation that
    # transpiles naturally to typed targets (Rust structs, Crystal
    # classes) while still being Rails-idiomatic in the source.

    def self.attr_accessor(*names)
      super
      @_schema_columns ||= []
      @_schema_columns.concat(names)
    end

    def self.schema_column_names
      @_schema_columns ||= []
    end

    def self.inherited(subclass)
      super
      # Each subclass gets its own column list — don't inherit the
      # parent's. (ApplicationRecord declares none; each model declares
      # its own.)
      subclass.instance_variable_set(:@_schema_columns, [])
    end

    # ── Class-level table metadata ───────────────────────────────────
    def self.table_name
      @table_name ||= begin
        base = name.to_s.downcase
        base.end_with?("s") ? base : "#{base}s"
      end
    end

    def self.table_name=(n)
      @table_name = n.to_s
    end

    def self.abstract?
      false
    end

    def self.instantiate(row)
      obj = allocate
      obj.send(:init_from_row, row)
      obj
    end

    def self.create(attrs = {})
      r = new(attrs)
      r.save
      r
    end

    def self.create!(attrs = {})
      r = new(attrs)
      raise RecordInvalid, r unless r.save
      r
    end

    def self.all
      ActiveRecord.adapter.all(table_name).map { |row| instantiate(row) }
    end

    def self.find(id)
      row = ActiveRecord.adapter.find(table_name, id)
      raise RecordNotFound, "Couldn't find #{name} with id=#{id}" unless row
      instantiate(row)
    end

    def self.find_by(conditions)
      row = ActiveRecord.adapter.where(table_name, conditions).first
      row ? instantiate(row) : nil
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
      all.each(&:destroy)
    end

    # ── Instance lifecycle ──────────────────────────────────────────
    def initialize(attrs = {})
      @errors = []
      @persisted = false
      @destroyed = false
      # Populate each schema ivar the attr_accessor generated. Using
      # `send("#{k}=", v)` routes through any override the model
      # provides (e.g. normalization, aliases) — matches Rails.
      attrs.each { |k, v| send("#{k}=", v) }
    end

    def attributes
      # Expose the current ivar state as a Hash for callers (adapter
      # interface, debugging). Built from the tracked column list so
      # internal ivars like @_comments stay out.
      self.class.schema_column_names.to_h { |c| [c, instance_variable_get("@#{c}")] }
    end

    def persisted?
      @persisted
    end

    def new_record?
      !@persisted
    end

    def destroyed?
      @destroyed == true
    end

    def read_attribute(name)
      instance_variable_get("@#{name}")
    end

    def [](name)
      instance_variable_get("@#{name}")
    end

    def []=(name, value)
      instance_variable_set("@#{name}", value)
    end

    # Lifecycle hooks — no-op defaults; transpiled models override
    # the ones they need.
    def before_validation; end
    def after_validation; end
    def before_save; end
    def after_save; end
    def before_create; end
    def after_create; end
    def before_update; end
    def after_update; end
    def before_destroy; end
    def after_destroy; end
    def after_commit; end
    def after_create_commit; end
    def after_update_commit; end
    def after_destroy_commit; end
    def after_save_commit; end
    def after_touch; end

    # Model's `validate` defines its own checks; default is empty.
    def validate; end

    def valid?
      @errors = []
      validate
      @errors.empty?
    end

    def save
      before_validation
      ok = valid?
      after_validation
      return false unless ok

      before_save
      new_record = new_record?
      if new_record
        before_create
        id = ActiveRecord.adapter.insert(self.class.table_name, attributes)
        @id = id if self.class.schema_column_names.include?(:id)
        @persisted = true
        after_create
      else
        before_update
        ActiveRecord.adapter.update(self.class.table_name, @id, attributes)
        after_update
      end
      after_save

      if new_record
        after_create_commit
      else
        after_update_commit
      end
      after_save_commit
      after_commit
      true
    end

    def save!
      save || raise(RecordInvalid, self)
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

    def update(attrs)
      attrs.each { |k, v| send("#{k}=", v) }
      save
    end

    def ==(other)
      other.is_a?(self.class) && !@id.nil? && @id == other[:id]
    end
    alias_method :eql?, :==

    def hash
      [self.class, @id].hash
    end

    private

    def init_from_row(row)
      @errors = []
      @persisted = true
      @destroyed = false
      self.class.schema_column_names.each do |col|
        instance_variable_set("@#{col}", row[col])
      end
    end
  end
end
