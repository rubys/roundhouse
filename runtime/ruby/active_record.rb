require_relative "active_record/errors"
require_relative "active_record/connection_pool"
require_relative "active_record/registry"
require_relative "active_record/base"
# After base: connection.rb reopens Base with the raw-SQL surface
# (`connection`/`transaction`); loading it first would define an empty
# Base the real one then clobbers.
require_relative "active_record/connection"
require_relative "active_record/arel"
require_relative "active_record/relation"
