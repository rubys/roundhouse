module ActiveRecord
  class RecordNotFound < StandardError
    # Rails' readers (activerecord 8.1.4): the model's class name, its
    # primary key and the id that was looked up, as passed. `find` sets
    # all three; `find_by!` / `first!` / `sole` set the model and key and
    # leave `id` nil. Apps read them in a `rescue_from` handler to build
    # the 404 body.
    attr_reader :model, :primary_key, :id

    # Rails' positional signature. The message default differs on
    # purpose: Ruby's StandardError sets the message to the class name
    # implicitly; JS Error doesn't, so spell the contract out so
    # transpiled targets get the same default.
    def initialize(message = "ActiveRecord::RecordNotFound", model = nil, primary_key = nil, id = nil)
      @model = model
      @primary_key = primary_key
      @id = id
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
  # RAISED BY EACH `Db`, not from here: the emitted per-model
  # `_adapter_insert` hands its SQL straight to the per-target `Db.exec`
  # primitive, and that is the only place holding the driver's error.
  # The three ruby-family `Db`s (`db.rb`, `db_cruby.rb`, `db_jruby.rb`)
  # each map it in `exec`, keyed on SQLITE'S OWN message text — "UNIQUE
  # constraint failed: <table>.<column>" comes out of the engine, so the
  # same string appears in the cruby gem's ConstraintException, in the
  # JDBC SQLException and in `sqlite3_errmsg`. The strict targets carry
  # their own `Db` and owe their own mapping.
  class RecordNotUnique < StandardError
    def initialize(message = "ActiveRecord::RecordNotUnique")
      super(message)
    end
  end

  # Rails raises this from `sole` / `find_sole_by` when more than one
  # record matches ("Wanted only one Widget"). Rails hangs it off
  # ActiveRecordError and passes the model class; with no such base
  # here, StandardError is the parent and the caller builds the
  # message, as for the classes above.
  class SoleRecordExceeded < StandardError
    def initialize(message = "Wanted only one record")
      super(message)
    end
  end

  # Raised by app code (`raise ActiveRecord::RecordNotSaved.new(msg, record)`) to answer 422 as Rails does; no runtime save path raises it yet.
  class RecordNotSaved < StandardError
    attr_reader :record

    def initialize(message = "ActiveRecord::RecordNotSaved", record = nil)
      @record = record
      super(message)
    end
  end

  # Raised by app code; no runtime path marks a record read-only yet.
  class ReadOnlyRecord < StandardError
    def initialize(message = "ActiveRecord::ReadOnlyRecord")
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
