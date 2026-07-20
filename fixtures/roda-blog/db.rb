# Database connection + schema.
#
# Sequel connects before any model class is defined (models subclass
# Sequel::Model, which needs a DB handle at class-definition time), and the
# migrations in db/migrate are run on boot so the app is runnable with no
# separate setup step.
require "sequel"

DB = Sequel.sqlite(ENV.fetch("DATABASE", File.expand_path("db/blog.db", __dir__)))

Sequel.extension :migration
Sequel::Migrator.run(DB, File.expand_path("db/migrate", __dir__))

# Behavior applied to every model.
#
# Sequel raises Sequel::ValidationFailed from #save on an invalid model by
# default (like ActiveRecord's #save!). Turning that off makes #save return
# nil/false on failure, so an `if model.save` branch validates exactly once
# (calling #valid? first and then #save would run validations twice).
Sequel::Model.raise_on_save_failure = false

Sequel::Model.plugin :validation_helpers          # explicit validations in #validate
Sequel::Model.plugin :timestamps, update_on_create: true
