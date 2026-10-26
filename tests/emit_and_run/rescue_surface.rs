//! Named request exceptions, default responses, and inherited `rescue_from` precedence.

use super::emit_and_run;

fn parse_error_predicate_app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app();
    for (index, body) in include_str!("parse_error_predicates.rb").split("\nclass ").enumerate() {
        let source = if index == 0 { body.to_string() } else { format!("class {body}") };
        let name = source.split_whitespace().nth(1).unwrap();
        let path = if name.ends_with("Controller") {
            format!("app/controllers/{}_controller.rb", name.trim_end_matches("Controller").to_lowercase())
        } else {
            format!("app/services/{}.rb", name.to_lowercase())
        };
        app = app.write(&path, &source);
    }
    let mut routes = "Rails.application.routes.draw do\n".to_string();
    for action in ["ancestry", "identity", "ancestry_raises", "identity_raises", "kind_raises", "class_raises",
        "deep", "match_false", "match_true", "match_raises", "bad_request", "parameter_missing", "exact"] {
        routes.push_str(&format!("  get '/predicates/{action}', to: 'predicates#{action}'\n"));
    }
    routes.push_str("end\n");
    app.write("config/routes.rb", &routes)
}

#[test]
fn parameters_parse_error_fallback_ignores_instance_class_predicates() {
    let script = format!(r#"
def predicate_response_request(action)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/predicates/#{{action}}",
                                        "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  body.close if body.respond_to?(:close)
  [status, nil]
rescue StandardError => error
  [500, error]
end
{}"#, include_str!("parse_error_predicates_contract.rb"));
    parse_error_predicate_app().run_ruby(&script).assert_passes();
}

#[test]
fn parameters_parse_error_cgi_fallback_ignores_instance_class_predicates() {
    let script = format!(r#"
def predicate_response_request(action)
  output = StringIO.new
  Main.run({{ "REQUEST_METHOD" => "GET", "PATH_INFO" => "/predicates/#{{action}}", "QUERY_STRING" => "" }},
           StringIO.new(""), output)
  [output.string[/\AStatus: (\d+)/, 1].to_i, nil]
rescue StandardError => error
  [500, error]
end
{}"#, include_str!("parse_error_predicates_contract.rb"));
    parse_error_predicate_app().run_ruby(&script).assert_passes();
}

fn parse_error_response_app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app();
    // Ingest each controller from its own file, as in an application.
    for (index, body) in include_str!("parse_error_responses.rb").split("\nclass ").enumerate() {
        let source = if index == 0 { body.to_string() } else { format!("class {body}") };
        let name = source.split_whitespace().nth(1).unwrap();
        let path = if name == "ParseFailure" {
            "app/services/parse_failure.rb".to_string()
        } else {
            format!("app/controllers/{}_controller.rb", name.trim_end_matches("Controller").to_lowercase())
        };
        app = app.write(&path, &source);
    }
    let mut routes = "Rails.application.routes.draw do\n".to_string();
    for controller in ["parsers", "handlers", "children", "broads", "unrelated", "causes", "raising", "reraises"] {
        for action in ["qualified", "absolute", "aliased", "absolute_alias", "constructed", "aliased_instance",
            "failed", "failed_instance", "empty_message", "nil_message", "false_message", "symbol_message",
            "integer_message", "implicit", "implicit_instance", "nested", "rethrown", "helper", "before", "after",
            "derived", "wrapped", "omitted_outside_rescue", "unrelated", "locally"] {
            routes.push_str(&format!("  get '/{controller}/{action}', to: '{controller}#{action}'\n"));
        }
    }
    routes.push_str("end\n");
    app.write("config/routes.rb", &routes)
}

#[test]
fn parameters_parse_error_default_response_preserves_application_handlers() {
    let script = format!(r#"
def parse_response_request(controller, action)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/#{{controller}}/#{{action}}",
                                        "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  body.close if body.respond_to?(:close)
  [status, nil]
rescue StandardError => error
  [500, error]
end
{}"#, include_str!("parse_error_responses_contract.rb"));
    parse_error_response_app().run_ruby(&script).assert_passes();
}

#[test]
fn parameters_parse_error_default_response_also_reaches_cgi() {
    let script = format!(r#"
def parse_response_request(controller, action)
  output = StringIO.new
  Main.run({{ "REQUEST_METHOD" => "GET", "PATH_INFO" => "/#{{controller}}/#{{action}}", "QUERY_STRING" => "" }},
           StringIO.new(""), output)
  status = output.string[/\AStatus: (\d+)/, 1].to_i
  [status, nil]
rescue StandardError => error
  [500, error]
end
{}"#, include_str!("parse_error_responses_contract.rb"));
    parse_error_response_app().run_ruby(&script).assert_passes();
}

#[test]
fn parameters_parse_error_exception_factory_keeps_its_unsupported_boundary() {
    let errors = emit_and_run::empty_app()
        .write("app/controllers/parses_controller.rb", r#"
class ParsesController < ActionController::API
  ParseAlias = ActionDispatch::Http::Parameters::ParseError
  def qualified
    raise ActionDispatch::Http::Parameters::ParseError.exception("bad JSON")
  end
  def aliased
    raise ParseAlias.exception("bad JSON")
  end
end
"#)
        .check_errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().all(|error| error.contains("no known method `exception`")), "{errors:?}");
}

#[test]
fn parameters_parse_error_preserves_the_active_exception_message() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/parses_controller.rb",
            r#"class ParsesController < ActionController::API
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :bad_json

  def show
    raise ArgumentError, "malformed input"
  rescue ArgumentError
    raise ActionDispatch::Http::Parameters::ParseError
  end

  def constructed
    raise ArgumentError, "constructed input"
  rescue ArgumentError
    raise ActionDispatch::Http::Parameters::ParseError.new
  end

  private

  def bad_json(error)
    render json: { error: error.message }, status: :bad_request
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get '/parse', to: 'parses#show'\n  get '/constructed', to: 'parses#constructed'\nend\n",
        )
        .run_ruby(
            r#"
{ "/parse" => "malformed input", "/constructed" => "constructed input" }.each do |path, message|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path,
                                        "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  body.close if body.respond_to?(:close)
  expected = [400, JSON.generate(error: message)]
  raise "parse response #{[status, text].inspect}" unless [status, text] == expected
end
"#,
        )
        .assert_passes();
}

#[test]
fn parameters_parse_error_constructor_contract_matches_rails() {
    emit_and_run::empty_app()
        .run_ruby(include_str!("parse_error_contract.rb"))
        .assert_passes();
}

#[test]
fn parameters_parse_error_resolves_and_reaches_the_bad_request_handler() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/parses_controller.rb",
            r#"class ParsesController < ApplicationController
  rescue_from ActionDispatch::Http::Parameters::ParseError, with: :bad_json

  def show
    raise ActionDispatch::Http::Parameters::ParseError, "bad JSON"
  end

  private

  def bad_json(error)
    render json: { error: error.message }, status: :bad_request
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get '/parse', to: 'parses#show'\nend\n",
        )
        .run_ruby(
            r#"
raise "exception ancestry" unless ActionDispatch::Http::Parameters::ParseError.superclass == StandardError
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/parse",
                                      "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
raise "parse response #{[status, text].inspect}" unless [status, text] == [400, %({"error":"bad JSON"})]
"#,
        )
        .assert_passes();
}

#[test]
fn rescue_from_prefers_the_subclass_then_its_last_declaration() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::API
  rescue_from ArgumentError, with: :parent_error
  rescue_from KeyError, with: :missing_key

  private

  def parent_error
    head :bad_request
  end

  def missing_key
    head :not_found
  end
end
"#,
        )
        .write(
            "app/controllers/parents_controller.rb",
            r#"class ParentsController < ApplicationController
  def index
    raise ArgumentError, "parent"
  end
end
"#,
        )
        .write(
            "app/controllers/children_controller.rb",
            r#"class ChildrenController < ParentsController
  rescue_from ArgumentError, with: :earlier_error
  rescue_from ArgumentError, with: :child_error

  def index
    raise ArgumentError, "child"
  end

  def missing
    raise KeyError, "key"
  end

  private

  def earlier_error
    head :unprocessable_entity
  end

  def child_error
    head :conflict
  end
end
"#,
        )
        .write(
            "app/controllers/grandchildren_controller.rb",
            r#"class GrandchildrenController < ChildrenController
  def index
    raise ArgumentError, "grandchild"
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get '/parent', to: 'parents#index'
  get '/child', to: 'children#index'
  get '/grandchild', to: 'grandchildren#index'
  get '/missing', to: 'children#missing'
end
"#,
        )
        .run_ruby(
            r##"
{ "/parent" => 400, "/child" => 409, "/grandchild" => 409, "/missing" => 404 }.each do |path, expected|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path,
                                        "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  body.close if body.respond_to?(:close)
  raise "#{path}: #{status}, expected #{expected}" unless status == expected
end
"##,
        )
        .assert_passes();
}
