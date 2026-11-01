class AliasRecord < ApplicationRecord
  self.table_name = "probe_records"
  attribute :comments_total, :integer
  def self.projected
    select("probe_records.*, '007' AS comments_total")
  end
end
