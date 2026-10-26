# The caller supplies predicate_response_request(action), returning
# [HTTP status, escaped exception]. Do not trust the error's own predicates.
checks = 0
{
  "ancestry" => ParseIsA,
  "identity" => ParseInstanceOf,
  "ancestry_raises" => ParseIsARaises,
  "identity_raises" => ParseInstanceOfRaises,
  "kind_raises" => ParseKindOfRaises,
  "class_raises" => ParseClassRaises,
  "deep" => ParseDeep,
  "match_false" => ParseMatchFalse,
  "match_true" => ParseMatchTrue,
  "match_raises" => ParseMatchRaises
}.each do |action, expected|
  status, escaped = predicate_response_request(action)
  unless status == 500 && Kernel.instance_method(:instance_of?).bind_call(escaped, expected)
    raise "#{action}: expected the original unmapped exception, got HTTP #{status}"
  end
  raise "#{action}: message changed" unless escaped.message == "bad JSON"
  checks += 1
end

%w[bad_request parameter_missing exact].each do |action|
  status, escaped = predicate_response_request(action)
  raise "#{action}: expected HTTP 400, got #{status}" unless status == 400 && escaped.nil?
  checks += 1
end

# Overrides on exact ParseError instances must not prevent their 400 either.
ActionDispatch::Http::Parameters::ParseError.class_eval do
  def is_a?(other)
    raise "is_a? called"
  end

  def instance_of?(other)
    raise "instance_of? called"
  end

  def class
    raise "class called"
  end
end
status, escaped = predicate_response_request("exact")
raise "exact with overrides: expected HTTP 400" unless status == 400 && escaped.nil?
checks += 1
puts "PASS #{checks} exception predicate cases"
