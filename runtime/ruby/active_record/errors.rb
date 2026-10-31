# frozen_string_literal: true
module ActiveModel
  # Rails' (activemodel 8.1.4) `initialize(record, attribute)` builds its
  # message from `record.class.name`; this file transpiles into the strict
  # runtimes, which have no `class` reflection. So the caller builds the
  # message, as for the ActiveRecord classes below: the Ruby targets'
  # `_unknown_mass_attribute` (connection.rb) passes Rails' text.
  class UnknownAttributeError < NoMethodError
    attr_reader :record, :attribute

    def initialize(record, attribute, message = "ActiveModel::UnknownAttributeError")
      @record = record
      @attribute = attribute
      super(message)
    end
  end
end

module ActiveRecord
  # Not StandardError subclasses each: Rails' hierarchy (8.1.4), so `rescue ActiveRecord::StatementInvalid` catches what Rails' does.
  class ActiveRecordError < StandardError
    def initialize(message = "ActiveRecord::ActiveRecordError")
      super(message)
    end
  end

  class StatementInvalid < ActiveRecordError
    def initialize(message = "ActiveRecord::StatementInvalid")
      super(message)
    end
  end

  class WrappedDatabaseException < StatementInvalid
    def initialize(message = "ActiveRecord::WrappedDatabaseException")
      super(message)
    end
  end

  class InvalidForeignKey < WrappedDatabaseException
    def initialize(message = "ActiveRecord::InvalidForeignKey")
      super(message)
    end
  end

  class NotNullViolation < StatementInvalid
    def initialize(message = "ActiveRecord::NotNullViolation")
      super(message)
    end
  end

  class CheckViolation < StatementInvalid
    def initialize(message = "ActiveRecord::CheckViolation")
      super(message)
    end
  end

  class ExclusionViolation < StatementInvalid
    def initialize(message = "ActiveRecord::ExclusionViolation")
      super(message)
    end
  end

  class RangeError < StatementInvalid
    def initialize(message = "ActiveRecord::RangeError")
      super(message)
    end
  end

  class TransactionRollbackError < StatementInvalid
    def initialize(message = "ActiveRecord::TransactionRollbackError")
      super(message)
    end
  end

  class SerializationFailure < TransactionRollbackError
    def initialize(message = "ActiveRecord::SerializationFailure")
      super(message)
    end
  end

  class Deadlocked < TransactionRollbackError
    def initialize(message = "ActiveRecord::Deadlocked")
      super(message)
    end
  end

  class QueryAborted < StatementInvalid
    def initialize(message = "ActiveRecord::QueryAborted")
      super(message)
    end
  end

  class LockWaitTimeout < StatementInvalid
    def initialize(message = "ActiveRecord::LockWaitTimeout")
      super(message)
    end
  end

  class StatementTimeout < QueryAborted
    def initialize(message = "ActiveRecord::StatementTimeout")
      super(message)
    end
  end

  class QueryCanceled < QueryAborted
    def initialize(message = "ActiveRecord::QueryCanceled")
      super(message)
    end
  end

  # Raised by Rails only while writes are prevented (`while_preventing_writes`, a reading role), which nothing here turns on.
  class ReadOnlyError < ActiveRecordError
    def initialize(message = "ActiveRecord::ReadOnlyError")
      super(message)
    end
  end

  class RecordNotFound < ActiveRecordError
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
  # reaches the DB (lobsters' Keystore.validate_input_key).
  class ValueTooLong < StatementInvalid
    def initialize(message = "ActiveRecord::ValueTooLong")
      super(message)
    end
  end

  # Rails raises this when an INSERT/UPDATE trips a unique index. Apps
  # rescue it to turn a lost race into a redirect rather than a 500 —
  # campfire's first-run screen does exactly that (two people opening a
  # brand-new install at once; the loser is sent to the root path).
  #
  # RAISED BY EACH `Db`, not from here: the emitted per-model
  # `_adapter_insert` hands its SQL straight to the per-target `Db.exec`
  # primitive, and that is the only place holding the driver's error.
  # The three ruby-family `Db`s (`db.rb`, `db_cruby.rb`, `db_jruby.rb`)
  # each map it (with the other statement errors, `Db.statement_error`),
  # keyed on SQLITE'S OWN message text — "UNIQUE constraint failed:
  # <table>.<column>" comes out of the engine, so the same string appears
  # in the cruby gem's ConstraintException, in the JDBC SQLException and
  # in `sqlite3_errmsg`. The strict targets carry their own `Db` and owe
  # their own mapping.
  class RecordNotUnique < WrappedDatabaseException
    def initialize(message = "ActiveRecord::RecordNotUnique")
      super(message)
    end
  end

  # Rails raises this from `sole` / `find_sole_by` when more than one
  # record matches ("Wanted only one Widget"). Rails hangs it off
  # ActiveRecordError and passes the model class; with no such base
  # here, StandardError is the parent and the caller builds the
  # message, as for the classes above.
  class SoleRecordExceeded < ActiveRecordError
    def initialize(message = "Wanted only one record")
      super(message)
    end
  end

  # Raised by app code (`raise ActiveRecord::RecordNotSaved.new(msg, record)`) to answer 422 as Rails does, and by the Ruby targets' `save!` when a callback throws `:abort`.
  class RecordNotSaved < ActiveRecordError
    attr_reader :record

    def initialize(message = "ActiveRecord::RecordNotSaved", record = nil)
      @record = record
      super(message)
    end
  end

  # Raised by app code (`raise ActiveRecord::RecordNotDestroyed.new(msg, record)`),
  # and by the Ruby targets' `destroy!` when a `before_destroy` throws `:abort`.
  class RecordNotDestroyed < ActiveRecordError
    attr_reader :record

    def initialize(message = "ActiveRecord::RecordNotDestroyed", record = nil)
      @record = record
      super(message)
    end
  end

  # Raised saving or destroying a read-only record (connection.rb), or by app code.
  class ReadOnlyRecord < ActiveRecordError
    def initialize(message = "ActiveRecord::ReadOnlyRecord")
      super(message)
    end
  end

  class RecordInvalid < ActiveRecordError
    attr_reader :record

    def initialize(record)
      @record = record
      super("Validation failed: #{record.errors.join(', ')}")
    end
  end
end
