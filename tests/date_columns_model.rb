class CalendarEntry < ApplicationRecord
  def shifted(months)
    due_on&.>>(months)
  end

  def shifted_index(months)
    self[:due_on]&.>>(months)
  end

  def shifted_attribute(months)
    read_attribute(:due_on)&.>>(months)
  end

  def reset_date
    self.due_on = Date.new(2024, 1, 31)
    shifted(2)
  end

  def parsed_date
    Date.iso8601("2024-02-29") >> 12
  end
end
