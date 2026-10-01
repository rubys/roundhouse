# Upstream's Renderer unit-test pattern: protocol doubles, not Rails app boot.
config = InertiaRails::Configuration.default
config.version = "asset-v1"
config.always_include_errors_hash = false
controller = Object.new
controller.define_singleton_method(:inertia_configuration) { config.bind_controller(self) }
controller.define_singleton_method(:inertia_view_assigns) { {} }
controller.define_singleton_method(:inertia_shared_data) { {app: "SyntheticApp"} }
controller.define_singleton_method(:inertia_collect_flash_data) { {} }
controller.define_singleton_method(:inertia_meta) { InertiaRails::MetaTagBuilder.new(self) }
controller.define_singleton_method(:session) { {} }
request_class = Struct.new(:headers, :original_fullpath) do
  def inertia?
    headers.key?("X-Inertia")
  end
end
response_class = Struct.new(:headers, :status) do
  def set_header(key, value)
    headers[key] = value
  end
end
request = request_class.new({"X-Inertia" => "true"}, "/articles/7")
response = response_class.new({}, 200)
calls = []
capture = ->(**options) { calls << options }
SurveyProbe.call(controller, request, response, capture)
expected = {"component" => "Articles/Show", "props" => {"app" => "SyntheticApp", "title" => "Synthetic", "active" => false, "missing" => nil}, "url" => "/articles/7", "version" => "asset-v1", "encryptHistory" => false, "clearHistory" => false, "sharedProps" => ["app"]}
raise "full page: #{calls.inspect}" unless calls.size == 1 && JSON.parse(calls[0][:json]) == expected
raise "protocol headers: #{response.headers.inspect}" unless response.headers == {"Vary" => "X-Inertia", "X-Inertia" => "true"}
request.headers.merge!("X-Inertia-Partial-Component" => "Articles/Show", "X-Inertia-Partial-Data" => "active")
calls.clear
SurveyProbe.call(controller, request, response, capture)
expected["props"] = {"active" => false}
raise "partial page: #{calls.inspect}" unless calls.size == 1 && JSON.parse(calls[0][:json]) == expected
app = ->(_env) { [302, {"Location" => "/next"}, []] }
delete = Rack::MockRequest.env_for("/items/7", "REQUEST_METHOD" => "DELETE", "HTTP_X_INERTIA" => "true")
post = Rack::MockRequest.env_for("/items/7", "REQUEST_METHOD" => "POST", "HTTP_X_INERTIA" => "true")
raise "DELETE redirect" unless SurveyProbe.redirect(app, delete)[0] == 303
raise "POST redirect" unless SurveyProbe.redirect(app, post)[0] == 302
puts "PASS Inertia full/partial false/null props+headers; DELETE/POST redirect boundary (no HTTP/app/DB)"
