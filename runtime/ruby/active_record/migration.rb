module ActiveRecord
  class Migration
    def self.[](_version)
      self
    end

    def change
      # User migrations override this.
    end

    def up
      change
    end

    def migrate
      up
    end

    def create_table(name, **_options, &block)
      builder = TableBuilder.new(name)
      block&.call(builder)
      ActiveRecord.adapter.create_table(
        name,
        columns: builder.columns,
        foreign_keys: builder.foreign_keys
      )
    end

    def drop_table(name)
      ActiveRecord.adapter.drop_table(name)
    end
  end

  class TableBuilder
    attr_reader :columns, :foreign_keys

    def initialize(name)
      @name = name
      @columns = [:id]
      @foreign_keys = []
    end

    def string(name, **_opts); @columns << name.to_sym; end
    def text(name, **_opts); @columns << name.to_sym; end
    def integer(name, **_opts); @columns << name.to_sym; end
    def bigint(name, **_opts); @columns << name.to_sym; end
    def float(name, **_opts); @columns << name.to_sym; end
    def decimal(name, **_opts); @columns << name.to_sym; end
    def boolean(name, **_opts); @columns << name.to_sym; end
    def date(name, **_opts); @columns << name.to_sym; end
    def datetime(name, **_opts); @columns << name.to_sym; end
    def time(name, **_opts); @columns << name.to_sym; end

    def timestamps(**_opts)
      @columns << :created_at
      @columns << :updated_at
    end

    def references(name, null: true, foreign_key: false, polymorphic: false, **_opts)
      fk_col = "#{name}_id".to_sym
      @columns << fk_col
      if polymorphic
        @columns << "#{name}_type".to_sym
      end
      if foreign_key
        target_table = "#{name}s"
        @foreign_keys << { column: fk_col, references: target_table }
      end
    end
    alias_method :belongs_to, :references

    def index(_cols, **_opts)
      # Index metadata is irrelevant for the in-memory adapter; record as no-op.
    end
  end
end
