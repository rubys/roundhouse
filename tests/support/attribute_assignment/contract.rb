record = AttributeRecord.create!(state: "saved", phase: "initial", exit_status: 3)
record.writer_order = ""
result = record.assign_attributes({"state" => "unsaved", :phase => "queued", "exit_status" => "9", :enabled => false, :note => nil})
raise "return value" unless result.nil?
raise "scalar attributes" unless record.state == "unsaved" && record.phase == "QUEUED" && record.exit_status == 9 && record.enabled == false
raise "writer order" unless record.writer_order == "phase:unsaved;"
raise "nil virtual" unless record.note.nil?
raise "wrote the database" unless AttributeRecord.find(record.id).state == "saved"
raise "pending change" unless record.pending_assignment?
record.assign_attributes(state: nil, exit_status: nil)
raise "nil clears fields" unless record.state.nil? && record.exit_status.nil?
record.assign_attributes({})
raise "empty hash mutated fields" unless record.state.nil? && record.phase == "QUEUED"
record.assign_attributes(started_at: Time.utc(2024, 2, 3, 4, 5, 6), metadata: {"count" => 2})
raise "temporal assignment" unless record.started_at.year == 2024
raise "json assignment" unless record.metadata["count"] == 2
record.assign_attributes(state: "finished")
record.save!
saved = AttributeRecord.find(record.id)
raise "save after assignment" unless saved.state == "finished" && saved.phase == "QUEUED"
raise "payload assignment" unless saved.update_from_payload({"state" => "running", "phase" => "compile", "exit_status" => nil}) == "running"
begin
  saved.assign_attributes(missing: "x")
  raise "unknown writer accepted"
rescue ActiveModel::UnknownAttributeError => error
  raise "unknown attribute metadata" unless error.attribute == "missing" && error.record.equal?(saved)
  # activemodel 8.1.4's text.
  raise "unknown attribute message: #{error.message}" unless error.message == "unknown attribute 'missing' for AttributeRecord."
end
begin
  saved.assign_attributes(deny: "x")
  raise "private writer accepted"
rescue ActiveModel::UnknownAttributeError
end
puts "attribute assignment passed"
