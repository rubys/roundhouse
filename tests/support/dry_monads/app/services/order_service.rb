class OrderService < BaseService
  def call(qty)
    checked = yield check(qty)
    total = yield price(checked)
    Success(total)
  end

  def pair(a, b)
    x, y = yield check(a), check(b)
    Success(x + y)
  end

  def rescued(qty)
    begin
      yield check(qty)
    rescue StandardError => e
      return Success("caught #{e.class}")
    end
    Success(:kept)
  end

  def nested(list)
    doubled = list.map { |q| yield(check(q)) * 2 }
    Success(doubled)
  end

  def unit
    Success()
  end

  def chain(qty)
    check(qty).bind { |v| Success(v * 2) }.fmap { |v| v + 1 }.or { |e| Failure("#{e}!") }
  end

  def describe(qty)
    check(qty).either(->(v) { "ok #{v}" }, ->(e) { "bad #{e}" })
  end

  def fallback(qty)
    check(qty).value_or { |e| e.to_s }
  end

  def signup(params)
    values = yield SignupContract.new.call(params)
    Success(values.to_h)
  end

  private

  def check(qty)
    qty.positive? ? Success(qty) : Failure(:negative)
  end

  def price(qty)
    qty > 10 ? Failure(:too_many) : Success(qty * 3)
  end
end
