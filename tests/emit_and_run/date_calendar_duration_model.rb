class CalendarDurationProbe
  def self.count
    6
  end

  def self.values
    [
      six_days_ago(count).iso8601,
      (Date.new(2024, 2, 28) + 1.day).iso8601,
      (Date.new(2024, 3, 1) - 1.day).iso8601,
      (Date.new(2024, 1, 1) - 2.weeks).iso8601,
      (Date.new(2024, 12, 31) + 1.week).iso8601,
      (Date.new(2024, 3, 31) - 1.month).iso8601,
      (Date.new(2024, 1, 31) + 1.months).iso8601,
      (Date.new(2024, 2, 29) - 1.year).iso8601,
      (Date.new(2024, 2, 29) + 4.years).iso8601,
      (Date.new(2024, 3, 1) - -2.days).iso8601,
      (Date.new(2024, 1, 31) + -1.month).iso8601
    ]
  end

  def self.six_days_ago(n)
    Date.new(2024, 3, 10) - n.days
  end
end
