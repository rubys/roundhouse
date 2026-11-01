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
observe("templates", TemplatesController)
