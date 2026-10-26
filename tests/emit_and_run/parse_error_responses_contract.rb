# The caller supplies parse_response_request(controller, action), returning
# [HTTP status, escaped exception]. Rails' ShowExceptions records the exception
# in the request env; the emitted caller records errors escaping the dispatcher.
checks = 0
plain = %w[qualified absolute aliased absolute_alias constructed aliased_instance failed failed_instance
           empty_message nil_message false_message symbol_message integer_message implicit implicit_instance
           nested rethrown helper before after]
cases = plain.map { |action| ["parsers", action, 400, nil] }
cases.concat([
  ["parsers", "derived", 500, ParseFailure],
  ["parsers", "wrapped", 500, RuntimeError],
  ["parsers", "omitted_outside_rescue", 500, NoMethodError],
  ["parsers", "unrelated", 500, ArgumentError],
  ["parsers", "locally", 202, nil]
])
%w[handlers children broads unrelated].each do |controller|
  expected = { "handlers" => 409, "children" => 410, "broads" => 422, "unrelated" => 400 }.fetch(controller)
  %w[qualified aliased constructed implicit before].each do |action|
    cases << [controller, action, expected, nil]
  end
end
cases.concat([
  ["handlers", "derived", 409, nil],
  ["handlers", "wrapped", 409, nil],
  ["children", "derived", 410, nil],
  ["children", "wrapped", 410, nil],
  ["broads", "derived", 422, nil],
  ["broads", "wrapped", 422, nil],
  ["broads", "unrelated", 422, nil],
  ["unrelated", "derived", 500, ParseFailure],
  ["unrelated", "wrapped", 500, RuntimeError],
  ["unrelated", "nested", 404, nil],
  ["unrelated", "after", 400, nil],
  ["causes", "qualified", 400, nil],
  ["causes", "implicit", 409, nil],
  ["causes", "implicit_instance", 409, nil],
  ["causes", "nested", 409, nil],
  ["raising", "unrelated", 400, nil],
  ["raising", "implicit", 400, nil],
  ["reraises", "qualified", 400, nil],
  ["reraises", "wrapped", 400, nil]
])
cases.each do |controller, action, expected, escaped_class|
  status, escaped = parse_response_request(controller, action)
  raise "#{controller}##{action}: HTTP #{status}, expected #{expected}" unless status == expected
  if escaped_class && !escaped.instance_of?(escaped_class)
    raise "#{controller}##{action}: escaped #{escaped.class}, expected #{escaped_class}"
  end
  checks += 1
end
raise "response case count changed: #{checks}" unless checks == 64
puts "PASS #{checks} ParseError response cases"
