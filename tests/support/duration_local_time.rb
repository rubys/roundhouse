class LocalDurationClock
  def self.added
    at = Time.utc(2026, 1, 2, 3, 4, 5, 123456)
    delay = 2.seconds
    at + delay
  end

  def self.subtracted
    at = Time.utc(2026, 1, 2, 3, 4, 5, 123456)
    delay = 2.minutes
    copy = delay
    at - copy
  end

  def self.retained_duration
    at = Time.utc(2026, 1, 2)
    delay = 2.hours
    later = at + delay
    raise "duration changed" unless delay.to_i == 7200
    later
  end

  def self.chained_added
    at = Time.utc(2026, 1, 2, 3, 4, 5, 123456)
    delay = copy = 2.seconds
    at + delay
  end

  def self.chained_subtracted
    at = Time.utc(2026, 1, 2, 3, 4, 5, 123456)
    delay = copy = 2.minutes
    at - copy
  end

  def self.captured
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    callback = -> { delay = 0.5 }
    delay = 2.seconds
    callback.call
    at + delay
  end

  def self.block_capture
    at = Time.utc(2026, 1, 2)
    delay = 2.seconds
    [0.5].each { |value| delay = value }
    delay = 2.seconds
    at + delay
  end
end
