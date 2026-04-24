module ActiveRecord
  @adapter = nil

  def self.adapter
    @adapter ||= InMemoryAdapter.new
  end

  def self.adapter=(a)
    @adapter = a
  end

  def self.reset_adapter
    @adapter = InMemoryAdapter.new
  end
end
