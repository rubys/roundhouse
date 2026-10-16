# frozen_string_literal: true
# One copy of Puma's illegal-header predicates for every Spinel HTTP/1.1
# writer (Tep servers and CGI). A key or value that cannot be one line is
# dropped, not rewritten.
module HttpHeaders
  def self.key_ok?(k)
    n = k.bytesize
    return false if n == 0
    i = 0
    while i < n
      b = k.getbyte(i)
      return false if b <= 32 || b == 127 || b == 34 || b == 58
      i += 1
    end
    true
  end

  def self.value_ok?(v)
    n = v.bytesize
    i = 0
    while i < n
      b = v.getbyte(i)
      return false if (b < 32 && b != 9) || b == 127
      i += 1
    end
    true
  end
end
