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

  def guard_assigned(flag)
    if !(g = (flag ? 1 : nil))
      return 0
    end
    g
  end

  def guard_assigned_compound(flag)
    if !(h = (flag ? 9 : nil)) || h < 5
      return 0
    end
    h
  end

  def assigned_or_default(flag)
    if !(d = (flag ? 1 : nil))
      d = 2
    end
    d
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
      probe.rescued(true), probe.rescued(false),
      probe.guard_assigned(true), probe.guard_assigned(false),
      probe.guard_assigned_compound(true), probe.guard_assigned_compound(false),
      probe.assigned_or_default(true), probe.assigned_or_default(false)
    ]
  end
end
