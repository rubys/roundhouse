module ActiveRecord
  class Base
    include Validations
    include Callbacks
    include Associations
    include Broadcasts

    @@class_registry = {}

    def self.inherited(subclass)
      super
      @@class_registry[subclass.name] = subclass
      subclass.instance_variable_set(:@schema_columns, nil)
    end

    def self.lookup_class(name)
      @@class_registry[name.to_s] || Object.const_get(name.to_s)
    end

    def self.primary_abstract_class
      @abstract_class = true
    end

    def self.abstract_class?
      @abstract_class == true
    end

    def self.table_name
      @table_name ||= begin
        base = name.to_s.downcase
        base.end_with?("s") ? base : "#{base}s"
      end
    end

    def self.table_name=(n)
      @table_name = n.to_s
    end

    def self.schema_columns
      @schema_columns ||= begin
        s = ActiveRecord.adapter.schema(table_name)
        s ? s[:columns].map(&:to_sym) : []
      end
    end

    def self.reset_schema_cache!
      @schema_columns = nil
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
      @persisted = false
      assign_attributes(attrs)
    end

    def assign_attributes(attrs)
      attrs.each { |k, v| @attributes[k.to_sym] = v }
    end

    def attributes
      @attributes.dup
    end

    def id
      @attributes[:id]
    end

    def persisted?
      @persisted
    end

    def new_record?
      !@persisted
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

    def save
      run_callbacks(:before_validation)
      unless valid?
        run_callbacks(:after_validation)
        return false
      end
      run_callbacks(:after_validation)

      return false unless foreign_keys_valid?

      run_callbacks(:before_save)
      new_record = new_record?
      if new_record
        run_callbacks(:before_create)
        id = ActiveRecord.adapter.insert(self.class.table_name, @attributes)
        @attributes[:id] = id
        @persisted = true
        run_callbacks(:after_create)
      else
        run_callbacks(:before_update)
        ActiveRecord.adapter.update(self.class.table_name, @attributes[:id], @attributes)
        run_callbacks(:after_update)
      end
      run_callbacks(:after_save)

      # Commit callbacks — in real AR these fire after transaction commit.
      # In-memory adapter has no transaction; fire them immediately.
      if new_record
        run_callbacks(:after_create_commit)
      else
        run_callbacks(:after_update_commit)
      end
      run_callbacks(:after_save_commit)
      run_callbacks(:after_commit)
      true
    end

    def save!
      save || raise(RecordInvalid, self)
    end

    def destroy
      return self unless persisted?
      run_callbacks(:before_destroy)
      destroy_dependents
      ActiveRecord.adapter.delete(self.class.table_name, @attributes[:id])
      @persisted = false
      @destroyed = true
      run_callbacks(:after_destroy)
      run_callbacks(:after_destroy_commit)
      run_callbacks(:after_commit)
      self
    end

    def destroyed?
      @destroyed == true
    end

    def update(attrs)
      assign_attributes(attrs)
      save
    end

    def ==(other)
      other.is_a?(self.class) && !id.nil? && id == other.id
    end
    alias_method :eql?, :==

    def hash
      [self.class, id].hash
    end

    # Dynamic attribute access. Looks up method_missing style so that
    # `article.title` and `article.title = "x"` work without explicit
    # attr_accessor — schema-driven.
    def method_missing(name, *args)
      str = name.to_s
      if str.end_with?("=") && args.size == 1
        @attributes[str.chomp("=").to_sym] = args.first
      elsif args.empty? && @attributes.key?(name)
        @attributes[name]
      elsif args.empty? && self.class.schema_columns.include?(name)
        @attributes[name]
      else
        super
      end
    end

    def respond_to_missing?(name, include_private = false)
      str = name.to_s
      if str.end_with?("=")
        true
      elsif @attributes.key?(name) || self.class.schema_columns.include?(name)
        true
      else
        super
      end
    end

    private

    def init_from_row(row)
      @attributes = row.dup
      @persisted = true
      @destroyed = false
      @errors = []
    end

    def foreign_keys_valid?
      self.class.associations.each do |_, assoc|
        next unless assoc[:kind] == :belongs_to
        next if assoc[:optional]
        fk_val = @attributes[assoc[:foreign_key]]
        if fk_val.nil?
          errors << "#{assoc[:foreign_key]} can't be blank"
          return false
        end
        target = ActiveRecord::Base.lookup_class(assoc[:target])
        unless ActiveRecord.adapter.exists?(target.table_name, fk_val)
          errors << "#{assoc[:target]} must exist"
          return false
        end
      end
      true
    end

    def destroy_dependents
      self.class.associations.each do |name, assoc|
        next unless assoc[:kind] == :has_many && assoc[:dependent] == :destroy
        send(name).each(&:destroy)
      end
    end
  end
end
