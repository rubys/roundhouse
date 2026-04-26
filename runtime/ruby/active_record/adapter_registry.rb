module ActiveRecord
  # Process-wide adapter slot. Concrete implementations install
  # themselves via `ActiveRecord.adapter = ...`. The framework refers
  # to whatever's installed through the AbstractAdapter interface;
  # raises if accessed before install.
  @adapter = nil

  def self.adapter
    @adapter || raise("ActiveRecord adapter not installed; assign ActiveRecord.adapter = <impl> before use")
  end

  def self.adapter=(a)
    @adapter = a
  end

  def self.reset_adapter
    @adapter = nil
  end
end
