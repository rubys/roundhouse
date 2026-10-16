# frozen_string_literal: true
# SQLite WAL checkpoint background thread — façade for Spinel AOT.
#
# Preview Campfire's real body uses `ActiveRecord::Base.connection_db_config`,
# `File::LOCK_*` / `flock`, and `FileUtils` — none of which Spinel models
# yet. Puma starts this on CRuby; the Spinel HTTP harness does not use
# Puma, so a no-op module keeps `app/models.rb` loading without refusing
# the AOT compile. CRuby restore puts the verbatim body back.

module SqliteWalCheckpoint
  INTERVAL = 0.25

  def self.start(_interval = INTERVAL)
    nil
  end

  def self.checkpoint
    nil
  end
end
