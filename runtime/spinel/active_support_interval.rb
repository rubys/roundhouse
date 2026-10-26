# frozen_string_literal: true
require_relative "active_support_duration"
# PostgreSQL `interval` columns. One copy: the spinel time-parsing file
# and the CRuby overlay that replaces it both `require_relative` this
# sibling, so the parser is not maintained twice.
#
# SQLite stores a whole-second INTEGER. The public value is
# `ActiveSupport::Duration`. ISO-8601 (`PT1H`, `-PT1H`) and Postgres
# renderings (`01:30:00`, `1 day 02:03:04`, `-1 days -02:00:00`) parse
# into that count. A fraction is kept only on a seconds field, and the
# fraction itself is truncated (`PT1.5S` → 1, `01:00:00.5` → 3600).
# A fraction on any larger unit is nil (`PT1.5H`). Anything else is
# nil, which is how Rails' interval caster treats a string it cannot
# parse.
module ActiveSupport
  def self.parse_db_interval(value)
    return nil if value.nil?
    return value if value.is_a?(ActiveSupport::Duration)
    return ActiveSupport::Duration.seconds(value) if value.is_a?(Integer)
    return nil unless value.is_a?(String)
    return nil if value.empty?
    if interval_integer_text?(value)
      return ActiveSupport::Duration.seconds(value.to_i)
    end
    seconds = interval_seconds(value)
    return nil if seconds.nil?
    ActiveSupport::Duration.seconds(seconds)
  end

  def self.format_db_interval(value)
    return nil if value.nil?
    return value.to_i if value.is_a?(ActiveSupport::Duration)
    return value if value.is_a?(Integer)
    return nil unless value.is_a?(String)
    return value.to_i if interval_integer_text?(value)
    seconds = interval_seconds(value)
    return nil if seconds.nil?
    seconds
  end

  def self.interval_integer_text?(str)
    return false if str.empty?
    i = 0
    if str[0, 1] == "-"
      return false if str.length == 1
      i = 1
    end
    while i < str.length
      c = str[i, 1]
      return false if c < "0" || c > "9"
      i = i + 1
    end
    true
  end

  def self.interval_seconds(str)
    if iso_interval?(str)
      sign = 1
      i = 0
      if str[0, 1] == "-"
        sign = -1
        i = 1
      end
      seconds = iso8601_interval_body(str, i)
      return nil if seconds.nil?
      return sign * seconds
    end
    postgres_interval_body(str, 0)
  end

  def self.iso_interval?(str)
    return true if str[0, 1] == "P"
    str.length > 1 && str[0, 1] == "-" && str[1, 1] == "P"
  end

  def self.iso8601_interval_body(str, i)
    return nil if str[i, 1] != "P"
    i = i + 1
    return nil if i >= str.length
    total = 0
    in_time = false
    saw = false
    while i < str.length
      if str[i, 1] == "T"
        return nil if in_time
        in_time = true
        i = i + 1
        return nil if i >= str.length
      else
        number = read_whole_number(str, i)
        return nil if number.nil?
        i = number[0]
        if i < str.length && str[i, 1] == "."
          j = i + 1
          frac = false
          while j < str.length
            c = str[j, 1]
            break if c < "0" || c > "9"
            frac = true
            j = j + 1
          end
          return nil unless frac
          return nil if j >= str.length || !in_time || str[j, 1] != "S"
          i = j
        end
        return nil if i >= str.length
        unit = iso_interval_unit(str[i, 1], in_time)
        return nil if unit.nil?
        total = total + number[1] * unit
        saw = true
        i = i + 1
      end
    end
    return nil unless saw
    total
  end

  def self.iso_interval_unit(designator, in_time)
    if in_time
      return ActiveSupport::Duration::SECONDS_PER_HOUR if designator == "H"
      return ActiveSupport::Duration::SECONDS_PER_MINUTE if designator == "M"
      return 1 if designator == "S"
      return nil
    end
    return ActiveSupport::Duration::SECONDS_PER_YEAR if designator == "Y"
    return ActiveSupport::Duration::SECONDS_PER_MONTH if designator == "M"
    return ActiveSupport::Duration::SECONDS_PER_WEEK if designator == "W"
    return ActiveSupport::Duration::SECONDS_PER_DAY if designator == "D"
    nil
  end

  def self.postgres_interval_body(str, i)
    total = 0
    saw = false
    while i < str.length
      while i < str.length && str[i, 1] == " "
        i = i + 1
      end
      break if i >= str.length
      sign = 1
      if str[i, 1] == "+" || str[i, 1] == "-"
        sign = -1 if str[i, 1] == "-"
        i = i + 1
      end
      number = read_whole_number(str, i)
      return nil if number.nil?
      i = number[0]
      if i < str.length && str[i, 1] == ":"
        clock = read_interval_clock(str, i, number[1])
        return nil if clock.nil?
        i = clock[0]
        total = total + sign * clock[1]
        saw = true
      else
        while i < str.length && str[i, 1] == " "
          i = i + 1
        end
        word = read_interval_word(str, i)
        return nil if word.nil?
        unit = postgres_interval_unit(word[1])
        return nil if unit.nil?
        total = total + sign * number[1] * unit
        saw = true
        i = word[0]
      end
    end
    return nil unless saw
    total
  end

  def self.read_interval_clock(str, i, hours)
    return nil if str[i, 1] != ":"
    i = i + 1
    mins = read_whole_number(str, i)
    return nil if mins.nil?
    i = mins[0]
    return nil if i >= str.length || str[i, 1] != ":"
    i = i + 1
    secs = read_whole_number(str, i)
    return nil if secs.nil?
    i = secs[0]
    if i < str.length && str[i, 1] == "."
      i = i + 1
      frac = false
      while i < str.length
        c = str[i, 1]
        break if c < "0" || c > "9"
        frac = true
        i = i + 1
      end
      return nil unless frac
    end
    seconds = hours * ActiveSupport::Duration::SECONDS_PER_HOUR
    seconds = seconds + mins[1] * ActiveSupport::Duration::SECONDS_PER_MINUTE
    seconds = seconds + secs[1]
    [i, seconds]
  end

  def self.read_interval_word(str, i)
    start = i
    while i < str.length
      c = str[i, 1]
      break if c < "a" || c > "z"
      i = i + 1
    end
    return nil if i == start
    [i, str[start, i - start]]
  end

  def self.postgres_interval_unit(word)
    return ActiveSupport::Duration::SECONDS_PER_YEAR if word == "year" || word == "years"
    return ActiveSupport::Duration::SECONDS_PER_MONTH if word == "mon" || word == "mons" || word == "month" || word == "months"
    return ActiveSupport::Duration::SECONDS_PER_WEEK if word == "week" || word == "weeks"
    return ActiveSupport::Duration::SECONDS_PER_DAY if word == "day" || word == "days"
    return ActiveSupport::Duration::SECONDS_PER_HOUR if word == "hour" || word == "hours"
    return ActiveSupport::Duration::SECONDS_PER_MINUTE if word == "minute" || word == "minutes" || word == "min" || word == "mins"
    return 1 if word == "second" || word == "seconds" || word == "sec" || word == "secs"
    nil
  end

  def self.read_whole_number(str, i)
    return nil if i >= str.length
    c = str[i, 1]
    return nil if c < "0" || c > "9"
    n = 0
    while i < str.length
      c = str[i, 1]
      break if c < "0" || c > "9"
      n = n * 10 + c.to_i
      i = i + 1
    end
    [i, n]
  end
end
