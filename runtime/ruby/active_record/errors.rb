module ActiveRecord
  class RecordNotFound < StandardError
    # Bare-construct default. Ruby's StandardError sets the message to
    # the class name implicitly; JS Error doesn't, so spell the
    # contract out so transpiled targets get the same default.
    def initialize(message = "ActiveRecord::RecordNotFound")
      super(message)
    end
  end

  class RecordInvalid < StandardError
    attr_reader :record

    def initialize(record)
      @record = record
      super("Validation failed: #{record.errors.join(', ')}")
    end
  end
end
