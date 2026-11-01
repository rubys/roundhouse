require "json"
require "stringio"
require "uri"
def observe(label, klass)
  path = "/" + label
  if defined?(Main)
    status, headers, response = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path,
      "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.url_scheme" => "http",
      "rack.input" => StringIO.new(""))
  else
    status, headers, response = klass.action(:index).call(Rack::MockRequest.env_for("http://example.org" + path))
  end
  body = +""
  response.each { |part| body << part }
  response.close if response.respond_to?(:close)
  selected = {}
  headers.each { |key, value| selected[key.downcase] = value if ["x-before", "x-after", "x-observed", "location"].include?(key.downcase) }
  selected["location"] = URI.parse(selected["location"]).request_uri if selected["location"]&.start_with?("http")
  decoded = begin JSON.parse(body); rescue JSON::ParserError; body; end
  puts JSON.generate({case: label, value: [status, decoded, selected]})
rescue => error
  puts JSON.generate({case: label, error: error.class.name, message: error.message})
end
observe("pages", PagesController)
observe("named_pages", NamedPagesController)
observe("inherited_pages", InheritedPagesController)
observe("rescue_pages", RescuePagesController)
observe("other_pages", OtherPagesController)
observe("encoded", EncodedController)
