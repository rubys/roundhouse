# frozen_string_literal: true
# PostgreSQL SQLSTATE codes to the ActiveRecord exceptions apps rescue,
# shared by the PostgreSQL Db shims (roundhouse#91). Both drivers (the pg
# gem and spinel-pg) expose the code the same way:
#
#   rescue PG::Error => e
#     PgErrors.raise_mapped(e.result.error_field(PG::PG_DIAG_SQLSTATE).to_s, e.message)
#     raise
#
# The rescue must be typed (`PG::Error`), not bare: on Spinel a generic
# rescue dispatches `result` to the wrong accessor (spinel-pg README).
#
# The table is Rails 8.1's PostgreSQL adapter translation, limited to the
# classes this runtime defines (runtime/ruby/active_record/errors.rb).
# A code with no class here is StatementInvalid, as Rails wraps it.
module PgErrors
  UNIQUE_VIOLATION = "23505"
  STRING_DATA_RIGHT_TRUNCATION = "22001"
  NUMERIC_VALUE_OUT_OF_RANGE = "22003"
  NOT_NULL_VIOLATION = "23502"
  FOREIGN_KEY_VIOLATION = "23503"
  CHECK_VIOLATION = "23514"
  EXCLUSION_VIOLATION = "23P01"
  SERIALIZATION_FAILURE = "40001"
  DEADLOCK_DETECTED = "40P01"
  LOCK_NOT_AVAILABLE = "55P03"
  QUERY_CANCELED = "57014"

  # Raises the mapped exception for `sqlstate`; any other code is Rails' StatementInvalid.
  def self.raise_mapped(sqlstate, message)
    raise ActiveRecord::RecordNotUnique, message if sqlstate == UNIQUE_VIOLATION
    raise ActiveRecord::ValueTooLong, message if sqlstate == STRING_DATA_RIGHT_TRUNCATION
    raise ActiveRecord::RangeError, message if sqlstate == NUMERIC_VALUE_OUT_OF_RANGE
    raise ActiveRecord::NotNullViolation, message if sqlstate == NOT_NULL_VIOLATION
    raise ActiveRecord::InvalidForeignKey, message if sqlstate == FOREIGN_KEY_VIOLATION
    raise ActiveRecord::CheckViolation, message if sqlstate == CHECK_VIOLATION
    raise ActiveRecord::ExclusionViolation, message if sqlstate == EXCLUSION_VIOLATION
    raise ActiveRecord::SerializationFailure, message if sqlstate == SERIALIZATION_FAILURE
    raise ActiveRecord::Deadlocked, message if sqlstate == DEADLOCK_DETECTED
    raise ActiveRecord::LockWaitTimeout, message if sqlstate == LOCK_NOT_AVAILABLE
    raise ActiveRecord::QueryCanceled, message if sqlstate == QUERY_CANCELED
    raise ActiveRecord::StatementInvalid, message
  end
end
