class BranchLocalJoin
  def if_only(flag)
    if flag
      x = 1
    end
    x
  end

  def if_else(flag)
    if flag
      y = 1
    else
      y = "s"
    end
    y
  end

  def prior_widens(flag)
    w = 0
    if flag
      w = "s"
    end
    w
  end

  def else_returns(flag)
    if flag
      v = 1
    else
      return 0
    end
    v
  end

  def case_without_else(flag)
    case flag
    when true then c = 1
    end
    c
  end

  def modifier_unless(flag)
    u = 5 unless flag
    u
  end

  def while_body(flag)
    i = 0
    while flag && i < 3
      last = i
      i += 1
    end
    last
  end

  def and_right(flag)
    flag && (q = 7)
    q
  end

  def rescued(flag)
    begin
      r = 1
      raise "boom" if flag
      r = "s"
    rescue
      r
    end
  end

  def self.values
    probe = new
    [
      probe.if_only(true), probe.if_only(false),
      probe.if_else(true), probe.if_else(false),
      probe.prior_widens(true), probe.prior_widens(false),
      probe.else_returns(true), probe.else_returns(false),
      probe.case_without_else(true), probe.case_without_else(false),
      probe.modifier_unless(true), probe.modifier_unless(false),
      probe.while_body(true), probe.while_body(false),
      probe.and_right(true), probe.and_right(false),
      probe.rescued(true), probe.rescued(false)
    ]
  end
end
