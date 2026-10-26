class ParseIsA < ActionDispatch::Http::Parameters::ParseError
  def is_a?(other)
    false
  end
end

class ParseInstanceOf < ActionDispatch::Http::Parameters::ParseError
  def instance_of?(other)
    true
  end
end

class ParseIsARaises < ActionDispatch::Http::Parameters::ParseError
  def is_a?(other)
    raise "is_a? called"
  end
end

class ParseInstanceOfRaises < ActionDispatch::Http::Parameters::ParseError
  def instance_of?(other)
    raise "instance_of? called"
  end
end

class ParseKindOfRaises < ActionDispatch::Http::Parameters::ParseError
  def kind_of?(other)
    raise "kind_of? called"
  end
end

class ParseClassRaises < ActionDispatch::Http::Parameters::ParseError
  def class
    raise "class called"
  end
end

class ParseDeep < ParseInstanceOf
end

class ParseMatchFalse < ActionDispatch::Http::Parameters::ParseError
  def self.===(other)
    false
  end
end

class ParseMatchTrue < ActionDispatch::Http::Parameters::ParseError
  def self.===(other)
    true
  end
end

class ParseMatchRaises < ActionDispatch::Http::Parameters::ParseError
  def self.===(other)
    raise "subclass matcher called"
  end
end

class SiblingBadRequest < ActionController::BadRequest
  def is_a?(other)
    true
  end

  def instance_of?(other)
    false
  end
end

class SiblingParameterMissing < ActionController::ParameterMissing
  def is_a?(other)
    raise "is_a? called"
  end

  def instance_of?(other)
    raise "instance_of? called"
  end
end

class PredicatesController < ActionController::API
  def ancestry
    raise ParseIsA, "bad JSON"
  end

  def identity
    raise ParseInstanceOf, "bad JSON"
  end

  def ancestry_raises
    raise ParseIsARaises, "bad JSON"
  end

  def identity_raises
    raise ParseInstanceOfRaises, "bad JSON"
  end

  def kind_raises
    raise ParseKindOfRaises, "bad JSON"
  end

  def class_raises
    raise ParseClassRaises, "bad JSON"
  end

  def deep
    raise ParseDeep, "bad JSON"
  end

  def match_false
    raise ParseMatchFalse, "bad JSON"
  end

  def match_true
    raise ParseMatchTrue, "bad JSON"
  end

  def match_raises
    raise ParseMatchRaises, "bad JSON"
  end

  def bad_request
    raise SiblingBadRequest, "bad request"
  end

  def parameter_missing
    raise SiblingParameterMissing, "parameter"
  end

  def exact
    raise ActionDispatch::Http::Parameters::ParseError, "bad JSON"
  end
end
