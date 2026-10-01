class SurveysController < ActionController::Base
  def index
    writer = WriterRecord.new(23, "Mira")
    first = EntryRecord.new(41, "Draft", false, nil, writer)
    second = EntryRecord.new(42, "Published", true, "note", writer)
    [EntryResource.new(first).to_h, EntryResource.new(second).to_h]
  end
end
