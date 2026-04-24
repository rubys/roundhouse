module ActiveRecord
  class Base
    include Validations
    include Broadcasts

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

    def initialize(attrs = {})
      @attributes = {}
      @errors = []
      @persisted = false
      @destroyed = false
      assign_attributes(attrs)
    end

    def assign_attributes(attrs)
      attrs.each { |k, v| @attributes[k.to_sym] = v }
    end

    def attributes
      @attributes.dup
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
      @attributes[name.to_sym]
    end

    def [](name)
      @attributes[name.to_sym]
    end

    def []=(name, value)
      @attributes[name.to_sym] = value
    end

    # Lifecycle hooks — no-op defaults; transpiled models override
    # the ones they need. These are defined explicitly (not via
    # method_missing or registration DSL) so they're transpile-clean.
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
        id = ActiveRecord.adapter.insert(self.class.table_name, @attributes)
        @attributes[:id] = id
        @persisted = true
        after_create
      else
        before_update
        ActiveRecord.adapter.update(self.class.table_name, @attributes[:id], @attributes)
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
      ActiveRecord.adapter.delete(self.class.table_name, @attributes[:id])
      @persisted = false
      @destroyed = true
      after_destroy
      after_destroy_commit
      after_commit
      self
    end

    def update(attrs)
      assign_attributes(attrs)
      save
    end

    def ==(other)
      other.is_a?(self.class) && !@attributes[:id].nil? && @attributes[:id] == other[:id]
    end
    alias_method :eql?, :==

    def hash
      [self.class, @attributes[:id]].hash
    end

    private

    def init_from_row(row)
      @attributes = row.dup
      @errors = []
      @persisted = true
      @destroyed = false
    end
  end
end
