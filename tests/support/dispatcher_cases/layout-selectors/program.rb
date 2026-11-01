require "json"
require "stringio"
def observe(label, klass)
  if defined?(Main)
    status, headers, response = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/" + label,
      "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.url_scheme" => "http",
      "rack.input" => StringIO.new(""))
  else
    status, headers, response = klass.action(:index).call(Rack::MockRequest.env_for("http://example.org/" + label))
  end
  body = +""
  response.each { |part| body << part }
  response.close if response.respond_to?(:close)
  puts JSON.generate({case: label, value: [status, body, headers["x-step"] || headers["X-Step"]]})
rescue => error
  puts JSON.generate({case: label, error: error.class.name, message: error.message})
end
observe("named_parent", NamedParentController)
observe("named_child", NamedChildController)
observe("block_parent", BlockParentController)
observe("block_child", BlockChildController)
observe("guarded_context", GuardedContextController)
observe("rescued_context", RescuedContextController)
observe("implicit_context", ImplicitContextController)
observe("string_layout", StringLayoutController)
observe("symbol_layout", SymbolLayoutController)
observe("false_layout", FalseLayoutController)
observe("nil_layout", NilLayoutController)
observe("application_layout", ApplicationLayoutController)
observe("dynamic_layout", DynamicLayoutController)
observe("rescue_dynamic_layout", RescueDynamicLayoutController)
observe("rescue_nil_layout", RescueNilLayoutController)
observe("rescue_symbol_layout", RescueSymbolLayoutController)
observe("true_layout", TrueLayoutController)
