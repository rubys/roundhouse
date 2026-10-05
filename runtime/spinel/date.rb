# A deliberately small, program-defined Date for the Spinel target.
# This is not Ruby's stdlib `date` package. The supported API is the
# surface analyzed for Rails date columns; unsupported formats fail
# loudly instead of being interpreted as timestamps or guessed dates.
class Date
  class Error < ArgumentError
  end

  MONTHS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
  WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]
  SHORT_WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
  MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
  SHORT_MONTH_NAMES = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

  def initialize(year, month, day)
    unless year >= 1 && year <= 9999 && month >= 1 && month <= 12 && day >= 1 && day <= Date.month_length(year, month)
      raise Date::Error, "invalid date"
    end
    @year = year
    @month = month
    @day = day
  end

  def self.civil(year, month = 1, day = 1)
    Date.new(year, month, day)
  end

  def self.leap_year?(year)
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
  end

  def self.month_length(year, month)
    return 29 if month == 2 && Date.leap_year?(year)
    MONTHS[month - 1]
  end

  def self.iso8601(value)
    unless value.is_a?(String) && value.length == 10 && value[4, 1] == "-" && value[7, 1] == "-"
      raise Date::Error, "invalid date: #{value}"
    end
    year = value[0, 4].to_i
    month = value[5, 2].to_i
    day = value[8, 2].to_i
    date = Date.new(year, month, day)
    raise Date::Error, "invalid date: #{value}" unless date.iso8601 == value
    date
  rescue ArgumentError
    raise Date::Error, "invalid date: #{value}"
  end

  def self.parse(value, _comp = true)
    Date.iso8601(value)
  end

  def self.strptime(value, format)
    raise Date::Error, "unsupported date format" unless format == "%Y-%m-%d"
    Date.iso8601(value)
  end

  def self.today
    now = Time.now
    Date.new(now.year, now.month, now.day)
  end

  def year
    @year
  end

  def month
    @month
  end

  def mon
    @month
  end

  def day
    @day
  end

  def mday
    @day
  end

  def leap?
    Date.leap_year?(@year)
  end

  def wday
    offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4]
    year = @year
    year -= 1 if @month < 3
    (year + year / 4 - year / 100 + year / 400 + offsets[@month - 1] + @day) % 7
  end

  def yday
    total = @day
    month = 1
    while month < @month
      total += Date.month_length(@year, month)
      month += 1
    end
    total
  end

  def sunday?
    wday == 0
  end

  def monday?
    wday == 1
  end

  def tuesday?
    wday == 2
  end

  def wednesday?
    wday == 3
  end

  def thursday?
    wday == 4
  end

  def friday?
    wday == 5
  end

  def saturday?
    wday == 6
  end

  def >>(months)
    shift_months(months)
  end

  def <<(months)
    shift_months(-months)
  end

  def shift_months(amount)
    index = @year * 12 + @month - 1 + amount
    year = index / 12
    month = index % 12 + 1
    year -= 1 if month == 0
    month = 12 if month == 0
    day = @day
    last = Date.month_length(year, month)
    day = last if day > last
    Date.new(year, month, day)
  end

  def to_date
    self
  end

  def to_time
    Time.local(@year, @month, @day, 0, 0, 0)
  end

  def iso8601
    format("%04d-%02d-%02d", @year, @month, @day)
  end

  def to_s
    iso8601
  end

  def inspect
    iso8601
  end

  def xmlschema
    iso8601
  end

  def as_json(_options = {})
    iso8601
  end

  def strftime(pattern)
    result = ""
    index = 0
    while index < pattern.length
      char = pattern[index, 1]
      if char == "%"
        index += 1
        code = pattern[index, 1]
        value = case code
        when "%" then "%"
        when "Y" then format("%04d", @year)
        when "m" then format("%02d", @month)
        when "d" then format("%02d", @day)
        when "e" then format("%2d", @day)
        when "F" then iso8601
        when "j" then format("%03d", yday)
        when "w" then wday.to_s
        when "u" then (wday == 0 ? 7 : wday).to_s
        when "a" then SHORT_WEEKDAYS[wday]
        when "A" then WEEKDAYS[wday]
        when "b", "h" then SHORT_MONTH_NAMES[@month - 1]
        when "B" then MONTH_NAMES[@month - 1]
        else raise Date::Error, "unsupported strftime directive: %#{code}"
        end
        result += value
      else
        result += char
      end
      index += 1
    end
    result
  end

  def <=>(other)
    return nil unless other.is_a?(Date)
    return -1 if @year < other.year
    return 1 if @year > other.year
    return -1 if @month < other.month
    return 1 if @month > other.month
    return -1 if @day < other.day
    return 1 if @day > other.day
    0
  end

  def ==(other)
    (self <=> other) == 0
  end

  def <(other)
    (self <=> other) == -1
  end

  def <=(other)
    (self <=> other) <= 0
  end

  def >(other)
    (self <=> other) == 1
  end

  def >=(other)
    (self <=> other) >= 0
  end
end
