module ActiveRecord
  class RecordNotFound < StandardError
  end

  class RecordInvalid < StandardError
    attr_reader :record

    def initialize(record)
      @record = record
      super("Validation failed: #{record.errors.join(', ')}")
    end
  end
end
