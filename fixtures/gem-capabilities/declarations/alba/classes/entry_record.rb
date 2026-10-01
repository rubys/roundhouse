class EntryRecord
  attr_reader :id, :title, :active, :note, :writer

  def initialize(id, title, active, note, writer)
    @id = id
    @title = title
    @active = active
    @note = note
    @writer = writer
  end
end
