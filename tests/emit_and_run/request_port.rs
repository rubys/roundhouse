use super::emit_and_run;

#[test]
fn emitted_controllers_read_the_request_port() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/port\", to: \"ports#show\"\n",
        )
        .write(
            "app/controllers/ports_controller.rb",
            r#"class PortsController < ApplicationController
  def show
    response.headers["X-Port"] = request.port.to_s
    head :ok
  end
end
"#,
        )
        .write(
            "test/controllers/ports_controller_test.rb",
            r#"require "test_helper"

class PortsControllerTest < ActionDispatch::IntegrationTest
  test "the controller reads the host and scheme ports" do
    host! "chat.test"
    get "/port"
    assert_equal "80", response.headers["x-port"]

    https!
    get "/port"
    assert_equal "443", response.headers["x-port"]

    host! "chat.test:3000"
    get "/port"
    assert_equal "3000", response.headers["x-port"]

    https! false
    host! "[::1]:8080"
    get "/port"
    assert_equal "8080", response.headers["x-port"]
  end

  test "the controller reads the forwarded host port" do
    host! "internal:3000"
    get "/port", headers: { "HTTP_X_FORWARDED_HOST" => "public.test:8443" }
    assert_equal "8443", response.headers["x-port"]
  end
end
"#,
        )
        .run_test("test/controllers/ports_controller_test.rb")
        .assert_passes();
}
