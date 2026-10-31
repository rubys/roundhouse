# frozen_string_literal: true
# Where runtime/spinel/db_pg.rb's two drivers differ, for spinel-pg
# (https://github.com/roundhouse-rb/spinel-pg) on Spinel. On CRuby the pg gem
# (libpq) drives db_pg.rb instead, and pg_driver_cruby.rb takes this
# file's place as pg_driver.rb. Everything else db_pg.rb asks of the
# driver is the API both share.
module PgDriver
  # A session; spinel-pg takes the settings positionally.
  def self.open(host, port, database, user, password)
    PG.connect(host, port, database, user, password)
  end

  # A result's command tag (`INSERT 0 1`).
  def self.command_tag(result)
    result.cmd_tag
  end

  # A driver error's SQLSTATE. spinel-pg keeps the server's diagnostics
  # on every error it raises, a lost session's included.
  def self.sqlstate(error)
    error.result.error_field(PG::PG_DIAG_SQLSTATE).to_s
  end
end
