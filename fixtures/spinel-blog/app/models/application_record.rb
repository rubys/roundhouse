require_relative "../../runtime/active_record"

# Application-level abstract base. Real-blog parity: each concrete model
# inherits from `ApplicationRecord`, not directly from `ActiveRecord::Base`.
# This level is where app-wide policies (auditing, soft-delete, etc.) would
# go if real-blog had any. Currently empty.
class ApplicationRecord < ActiveRecord::Base
  def self.abstract?
    true
  end
end
