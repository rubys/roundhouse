module ActiveRecord
  # Runtime helper for has_many associations. Transpiled models
  # instantiate this directly in their association getters:
  #
  #   def comments
  #     @_comments ||= ActiveRecord::CollectionProxy.new(
  #       owner: self, target_class: Comment, foreign_key: :article_id
  #     )
  #   end
  #
  # belongs_to and has_one are expanded inline by the transpiler —
  # they don't need a runtime helper.
  class CollectionProxy
    include Enumerable

    def initialize(owner:, target_class:, foreign_key:)
      @owner = owner
      @target_class = target_class
      @foreign_key = foreign_key
    end

    def to_a
      rows = ActiveRecord.adapter.where(
        @target_class.table_name,
        @foreign_key => @owner.id
      )
      rows.map { |r| @target_class.instantiate(r) }
    end

    def each(&block)
      to_a.each(&block)
    end

    def size
      ActiveRecord.adapter.where(
        @target_class.table_name,
        @foreign_key => @owner.id
      ).size
    end
    alias_method :length, :size
    alias_method :count, :size

    def empty?
      size == 0
    end

    def build(attrs = {})
      @target_class.new(attrs.merge(@foreign_key => @owner.id))
    end

    def create(attrs = {})
      record = build(attrs)
      record.save
      record
    end
  end
end
