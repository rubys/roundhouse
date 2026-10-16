class Worker
  include Dry::Monads[:result]

  def early(q)
    yield check(q)
  end

  include Dry::Monads::Do.for(:work)

  def work(q)
    v = yield check(q)
    "worked #{v}"
  rescue Dry::Monads::Do::Halt => e
    "halted #{e.result.failure}"
  end

  def other(q)
    yield check(q)
  end

  def check(q)
    q.positive? ? Success(q) : Failure(:negative)
  end
end

class ChildWorker < Worker
  def work(q)
    yield check(q)
  end
end

class SplitWorker
  include Dry::Monads[:result]
  include Dry::Monads::Do.for(:a)

  def b(q)
    yield check(q)
  end

  include Dry::Monads::Do.for(:b)

  def a(q)
    v = yield check(q)
    Success(v)
  end

  def check(q)
    q.positive? ? Success(q) : Failure(:negative)
  end
end
