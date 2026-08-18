require_relative "active_record/errors"
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
require_relative "active_record/arel"
require_relative "active_record/relation"
