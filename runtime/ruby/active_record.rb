# frozen_string_literal: true
require_relative "active_record/errors"
require_relative "active_record/connection_not_established"
require_relative "active_record/encryption_errors"
require_relative "active_record/connection_pool"
require_relative "active_record/registry"
require_relative "active_record/base"
# After base: connection.rb reopens Base with the raw-SQL surface
# (`connection`/`transaction`); loading it first would define an empty
# Base the real one then clobbers.
require_relative "active_record/connection"
# Signed ids (`record.signed_id` / `Model.find_signed`) — like
# connection.rb a reopen-tier file the strict targets do not stage: it
# calls into ActionController::MessageVerifier, whose PBKDF2/HMAC
# primitives ship only with the ruby-family trees.
require_relative "active_record/signed_id"
# has_secure_password's reset token — same tier as signed_id.
require_relative "active_record/token_for"
# friendly_id's finder rule (Relation#friendly_find).
require_relative "active_record/friendly_id"
require_relative "active_record/arel"
require_relative "active_record/relation"
require_relative "active_record/preloading"
require_relative "active_record/types"
# Database tasks (`db:schema:load`, `prepare_all`, `migrate`). After
# connection.rb: the task runner calls `Connection`. The readers load
# before `tasks.rb`, which prepends the app rake file onto the methods
# they defined.
require_relative "rake/task"
require_relative "active_record/schema_reader"
require_relative "active_record/migration_runner"
require_relative "active_record/tasks"
