module ActiveRecord
  module Associations
    def self.included(base)
      base.extend(ClassMethods)
    end

    module ClassMethods
      def associations
        @associations ||= {}
      end

      def inherited(subclass)
        super
        subclass.instance_variable_set(:@associations, associations.dup)
      end

      def has_many(name, dependent: nil, foreign_key: nil, class_name: nil)
        fk = foreign_key || "#{self.name.to_s.downcase}_id"
        klass = class_name || name.to_s.chomp("s").capitalize
        associations[name] = {
          kind: :has_many,
          target: klass,
          foreign_key: fk.to_sym,
          dependent: dependent
        }
        define_method(name) { CollectionProxy.new(self, self.class.associations[name]) }
      end

      def has_one(name, foreign_key: nil, class_name: nil)
        fk = foreign_key || "#{self.name.to_s.downcase}_id"
        klass = class_name || name.to_s.capitalize
        associations[name] = { kind: :has_one, target: klass, foreign_key: fk.to_sym }
        define_method(name) do
          target = ActiveRecord::Base.lookup_class(self.class.associations[name][:target])
          rows = ActiveRecord.adapter.where(
            target.table_name,
            self.class.associations[name][:foreign_key] => @attributes[:id]
          )
          rows.empty? ? nil : target.instantiate(rows.first)
        end
      end

      def belongs_to(name, optional: false, class_name: nil, foreign_key: nil)
        klass = class_name || name.to_s.capitalize
        fk = (foreign_key || "#{name}_id").to_sym
        associations[name] = {
          kind: :belongs_to,
          target: klass,
          foreign_key: fk,
          optional: optional
        }
        define_method(name) do
          assoc = self.class.associations[name]
          fk_val = @attributes[assoc[:foreign_key]]
          return nil if fk_val.nil?
          target = ActiveRecord::Base.lookup_class(assoc[:target])
          row = ActiveRecord.adapter.find(target.table_name, fk_val)
          row ? target.instantiate(row) : nil
        end
      end
    end
  end

  class CollectionProxy
    include Enumerable

    def initialize(owner, assoc)
      @owner = owner
      @assoc = assoc
    end

    def target_class
      ActiveRecord::Base.lookup_class(@assoc[:target])
    end

    def to_a
      rows = ActiveRecord.adapter.where(
        target_class.table_name,
        @assoc[:foreign_key] => @owner.id
      )
      rows.map { |r| target_class.instantiate(r) }
    end

    def each(&block)
      to_a.each(&block)
    end

    def size
      ActiveRecord.adapter.where(
        target_class.table_name,
        @assoc[:foreign_key] => @owner.id
      ).size
    end
    alias_method :length, :size
    alias_method :count, :size

    def empty?
      size == 0
    end

    def build(attrs = {})
      target_class.new(attrs.merge(@assoc[:foreign_key] => @owner.id))
    end

    def create(attrs = {})
      record = build(attrs)
      record.save
      record
    end
  end
end
