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

  # Rails raises this when an INSERT/UPDATE trips a unique index. Apps
  # rescue it to turn a lost race into a redirect rather than a 500 —
  # campfire's first-run screen does exactly that (two people opening a
  # brand-new install at once; the loser is sent to the root path).
  #
  # Rails hangs it off WrappedDatabaseException < StatementInvalid; with
  # no statement-error hierarchy here, StandardError is the honest
  # parent, matching `ValueTooLong` above.
  #
  # NOT YET RAISED by the adapters: `SqliteAdapter.insert` hands the SQL
  # to the per-target `Db.exec` primitive, whose error surface differs
  # per target (CRuby's sqlite3 gem, spinel's FFI, …), so mapping the
  # driver's constraint error onto this class belongs with each `Db`
  # rather than here. Defining it is still load-bearing: without the
  # constant, `rescue ActiveRecord::RecordNotUnique` raises NameError
  # when the rescue clause is EVALUATED, taking down the happy path too.
  class RecordNotUnique < StandardError
    def initialize(message = "ActiveRecord::RecordNotUnique")
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
