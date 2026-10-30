require_relative "../support/rails_oracle"
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
require_relative "schema"
class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
require_relative "article"
require_relative "comment"
require_relative "probe"
