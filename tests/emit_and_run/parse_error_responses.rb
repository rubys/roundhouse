class ParseFailure < ActionDispatch::Http::Parameters::ParseError
end

class ParsersController < ActionController::API
  ParseAlias = ActionDispatch::Http::Parameters::ParseError

  before_action :reject_input, only: :before
  after_action :reject_input, only: :after

  def qualified
    raise ActionDispatch::Http::Parameters::ParseError, "bad JSON"
  end

  def absolute
    raise ::ActionDispatch::Http::Parameters::ParseError, "bad JSON"
  end

  def aliased
    raise ParseAlias, "bad JSON"
  end

  def absolute_alias
    raise ::ParsersController::ParseAlias, "bad JSON"
  end

  def constructed
    raise ActionDispatch::Http::Parameters::ParseError.new("bad JSON")
  end

  def aliased_instance
    raise ParseAlias.new("bad JSON")
  end

  def failed
    fail ParseAlias, "bad JSON"
  end

  def failed_instance
    fail ParseAlias.new("bad JSON")
  end

  def empty_message
    raise ParseAlias, ""
  end

  def nil_message
    raise ParseAlias, nil
  end

  def false_message
    raise ParseAlias, false
  end

  def symbol_message
    raise ParseAlias, :invalid
  end

  def integer_message
    raise ParseAlias, 42
  end

  def implicit
    raise ArgumentError, "source error"
  rescue ArgumentError
    raise ParseAlias
  end

  def implicit_instance
    raise ArgumentError, "source error"
  rescue ArgumentError
    raise ParseAlias.new
  end

  def nested
    begin
      raise ArgumentError, "outer error"
    rescue ArgumentError
      raise KeyError, "inner error"
    end
  rescue KeyError
    raise ParseAlias.new
  end

  def rethrown
    raise ParseAlias, "bad JSON"
  rescue ParseAlias
    raise
  end

  def helper
    reject_input
  end

  def before
    head :ok
  end

  def after
    head :ok
  end

  def derived
    raise ParseFailure, "derived error"
  end

  def wrapped
    raise ParseAlias, "inner error"
  rescue ParseAlias
    raise RuntimeError, "outer error"
  end

  def omitted_outside_rescue
    raise ParseAlias
  end

  def unrelated
    raise ArgumentError, "unrelated error"
  end

  def locally
    raise ParseAlias, "bad JSON"
  rescue ParseAlias
    head :accepted
  end

  private

  def reject_input
    raise ParseAlias, "bad JSON"
  end
end

class HandlersController < ParsersController
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :parse_error

  private

  def parse_error
    head :conflict
  end
end

class ChildrenController < HandlersController
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :earlier
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :latest

  private

  def earlier
    head :forbidden
  end

  def latest
    head :gone
  end
end

class BroadsController < ParsersController
  rescue_from StandardError, with: :any_error

  private

  def any_error
    head :unprocessable_entity
  end
end

class UnrelatedController < ParsersController
  rescue_from KeyError, with: :missing_key

  private

  def missing_key
    head :not_found
  end
end

class CausesController < ParsersController
  rescue_from ArgumentError, with: :source_error

  private

  def source_error
    head :conflict
  end
end

class RaisingController < ParsersController
  rescue_from ArgumentError, with: :wrap_error

  private

  def wrap_error
    raise ActionDispatch::Http::Parameters::ParseError, "handler error"
  end
end

class ReraisesController < ParsersController
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :reraise

  private

  def reraise(error)
    raise error
  end
end
