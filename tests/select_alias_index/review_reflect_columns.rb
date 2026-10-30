class AliasRecord < ApplicationRecord
  self.table_name = "probe_records"
  def self.projected
    select("probe_records.*, 7 AS comments_total")
  end
end
