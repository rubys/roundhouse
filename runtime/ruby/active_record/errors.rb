module ActiveRecord
  class RecordNotFound < StandardError
    # Bare-construct default. Ruby's StandardError sets the message to
    # the class name implicitly; JS Error doesn't, so spell the
    # contract out so transpiled targets get the same default.
    def initialize(message = "ActiveRecord::RecordNotFound")
      super(message)
    end
  end

  # Rails raises this when a value exceeds its column's declared limit;
  # apps also construct it directly to reject over-long input before it
  # reaches the DB (lobsters' Keystore.validate_input_key). Rails hangs
  # it off StatementInvalid — with no statement-error hierarchy here,
  # StandardError is the honest parent.
  class ValueTooLong < StandardError
    def initialize(message = "ActiveRecord::ValueTooLong")
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
