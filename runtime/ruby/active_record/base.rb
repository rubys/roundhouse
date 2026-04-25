module ActiveRecord
  class Base
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
      self.class.schema_column_names.to_h { |c| [c, _read_ivar(c)] }
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
      _read_ivar(name)
    end

    def [](name)
      _read_ivar(name)
    end

    def []=(name, value)
      _write_ivar(name, value)
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

    # ── Validations (inlined from ActiveRecord::Validations) ─────────
    #
    # Previously a `module Validations` mixed into Base. Inlined into
    # Base because the body-typer can't follow `self`-as-includer for
    # module instance methods — `self.class.table_name` and similar
    # cross-method calls only resolve when self is concretely Base.

    def errors
      @errors ||= []
    end

    def validates_presence_of(attr)
      value = read_for_validation(attr)
      # belongs_to fallback: `validates_presence_of(:article)` checks the
      # `article_id` foreign key when there's no direct `article` column.
      fk_attr = "#{attr}_id"
      if value.nil? && _has_accessor?(fk_attr)
        value = _read_accessor(fk_attr)
      end
      errors << "#{attr} can't be blank" if blank?(value)
    end

    def validates_absence_of(attr)
      value = read_for_validation(attr)
      errors << "#{attr} must be blank" unless blank?(value)
    end

    def validates_length_of(attr, minimum: nil, maximum: nil, is: nil)
      value = read_for_validation(attr)
      return if value.nil?
      len = value.respond_to?(:length) ? value.length : 0
      errors << "#{attr} is too short (minimum is #{minimum})" if minimum && len < minimum
      errors << "#{attr} is too long (maximum is #{maximum})" if maximum && len > maximum
      errors << "#{attr} is the wrong length (should be #{is})" if is && len != is
    end

    def validates_numericality_of(attr, greater_than: nil, less_than: nil, only_integer: false)
      value = read_for_validation(attr)
      if value.nil? || !value.is_a?(Numeric)
        errors << "#{attr} is not a number"
        return
      end
      errors << "#{attr} must be greater than #{greater_than}" if greater_than && !(value > greater_than)
      errors << "#{attr} must be less than #{less_than}" if less_than && !(value < less_than)
      errors << "#{attr} must be an integer" if only_integer && !value.is_a?(Integer)
    end

    def validates_inclusion_of(attr, in:)
      set = binding.local_variable_get(:in)
      value = read_for_validation(attr)
      errors << "#{attr} is not included in the list" unless set.include?(value)
    end

    def validates_format_of(attr, with:)
      value = read_for_validation(attr)
      errors << "#{attr} is invalid" unless value.is_a?(String) && with.match?(value)
    end

    def validates_uniqueness_of(attr, scope: [], case_sensitive: true)
      value = read_for_validation(attr)
      table = self.class.table_name
      scope_attrs = Array(scope)
      matches = ActiveRecord.adapter.all(table).select do |row|
        row_val = row[attr.to_sym]
        same = if !case_sensitive && row_val.is_a?(String) && value.is_a?(String)
                 row_val.downcase == value.downcase
               else
                 row_val == value
               end
        same &&
          (!persisted? || row[:id] != @id) &&
          scope_attrs.all? { |s| row[s.to_sym] == _read_accessor(s) }
      end
      errors << "#{attr} has already been taken" unless matches.empty?
    end

    # ── Broadcasts (inlined from ActiveRecord::Broadcasts) ──────────
    #
    # Stub broadcasts log per call for test inspection; target-native
    # Turbo integration is a later concern. Module-level log state
    # lives on `ActiveRecord::Broadcasts` (the small log-holder class
    # this file no longer mixes in but keeps as state).

    def broadcast_replace_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :replace, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_append_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :append, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_prepend_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :prepend, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_remove_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :remove, record: self, channels: channels, target: target }
      nil
    end

    private

    def init_from_row(row)
      @errors = []
      @persisted = true
      @destroyed = false
      self.class.schema_column_names.each do |col|
        _write_ivar(col, row[col])
      end
    end

    def read_for_validation(attr)
      _has_accessor?(attr) ? _read_accessor(attr) : _read_ivar(attr)
    end

    def blank?(value)
      value.nil? || (value.respond_to?(:empty?) && value.empty?)
    end

    # ── Reflection chokepoints ──────────────────────────────────────
    #
    # Ruby's `instance_variable_get/set`, `send`, and `respond_to?`
    # are unavoidable for the cross-attribute helpers above (the
    # framework is generic over the model's column set). Funneling
    # them through these typed chokepoints means callers compile
    # cleanly and only the chokepoint bodies themselves are untypable.

    def _read_ivar(name)
      instance_variable_get("@#{name}")
    end

    def _write_ivar(name, value)
      instance_variable_set("@#{name}", value)
    end

    def _read_accessor(name)
      send(name)
    end

    def _has_accessor?(name)
      respond_to?(name)
    end
  end
end
