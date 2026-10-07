#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The generic route table includes both supported digit-regex spellings,
/// multiple constrained captures, and an unconstrained control.
fn constrained_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", r#"
class WidgetsController < ApplicationController
  def show
    render plain: params[:id].to_s
  end
end
"#)
        .write("config/routes.rb", r#"
Rails.application.routes.draw do
  get "/widgets/:id", to: "widgets#show", constraints: { id: /\d+/ }
  get "/tenants/:tenant_id/widgets/:id", to: "widgets#show", constraints: { tenant_id: /[0-9]+/, id: /\d+/ }
  get "/open/:id", to: "widgets#show"
end
"#)
        .write("test/controllers/widgets_controller_test.rb", r#"
class WidgetsControllerTest < ActionDispatch::IntegrationTest
  def test_valid_digit_capture
    get "/widgets/17"
    assert_response :success
  end
end
"#)
}

/// Both production and controller-test dispatch must receive the flattened
/// integer-capture metadata; a four-argument Route silently defaults it away.
#[test]
fn swift_route_tables_preserve_digit_constraints() {
    let (emitted, errors) = constrained_app().emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    for path in ["Sources/App/main.swift", "Tests/AppTests/TestSetup.swift"] {
        let source = std::fs::read_to_string(emitted.join(path)).unwrap();
        assert!(source.contains(r#"Route("GET", "/widgets/:id", "WidgetsController", "show", nil, "id")"#), "{path}: {source}");
        assert!(source.contains(r#"Route("GET", "/tenants/:tenant_id/widgets/:id", "WidgetsController", "show", nil, "tenant_id id")"#), "{path}: {source}");
        assert!(source.contains(r#"Route("GET", "/open/:id", "WidgetsController", "show")"#), "{path}: {source}");
    }
}

/// Run the whole emitted Swift app and its actual test route table. Percent
/// escapes are checked as raw constrained segments, before any path decoding.
#[test]
#[ignore = "requires Swift 6+, SQLite development headers and SPM dependencies"]
fn swift_digit_constraints_reject_nonmatching_captures() {
    let (emitted, errors) = constrained_app().emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("Tests/AppTests/RouteConstraintNativeTests.swift"), r#"
import XCTest
@testable import App

final class RouteConstraintNativeTests: RoundhouseTestCase {
    /// Use the generated Server boundary and the unchanged emitted route table.
    private func dispatch(_ path: String) -> DispatchResult {
        Server.dispatch("GET", path, [:], [:], [:], RoundhouseTestSetup.routes,
                        RoundhouseTestSetup.controllers, { body, _, _ in body })
    }

    /// Reject nonmatching raw segments while preserving valid and open routes.
    func testProductionDispatchKeepsDigitConstraints() {
        for path in ["/widgets/abc", "/widgets/+1", "/widgets/-1", "/widgets/1a5",
                     "/widgets/%31", "/widgets/%FF", "/tenants/no/widgets/7",
                     "/tenants/2/widgets/no", "/tenants/%32/widgets/7"] {
            let response = dispatch(path)
            XCTAssertEqual(response.status, 404, path)
            XCTAssertEqual(response.body, "Not Found", path)
        }
        for (path, body) in [("/widgets/001", "001"), ("/widgets/001.html", "001"),
                             ("/tenants/2/widgets/7", "7"), ("/open/abc", "abc")] {
            let response = dispatch(path)
            XCTAssertEqual(response.status, 200, path)
            XCTAssertEqual(response.body, body, path)
        }
    }

    /// Pin the second generated table and its normal successful request helpers.
    func testControllerTestTableKeepsEveryConstrainedKey() throws {
        for path in ["/widgets/abc", "/widgets/%31", "/widgets/%FF",
                     "/tenants/no/widgets/7", "/tenants/2/widgets/no"] {
            XCTAssertNil(try Router.match("GET", path, RoundhouseTestSetup.routes), path)
        }
        get("/widgets/17")
        XCTAssertEqual(__status, 200)
        XCTAssertEqual(__body, "17")
        get("/open/abc")
        XCTAssertEqual(__status, 200)
        XCTAssertEqual(__body, "abc")
    }
}
"#).unwrap();
    let output = std::process::Command::new("swift")
        .args(["test", "--jobs", "2", "--filter", "RouteConstraintNativeTests"])
        .current_dir(&emitted).output().expect("run generated Swift digit-constraint contract");
    std::fs::write(emitted.join("swift-test.stdout"), &output.stdout).unwrap();
    std::fs::write(emitted.join("swift-test.stderr"), &output.stderr).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "Swift digit-constraint contract at {}:\n{stdout}\n{stderr}", emitted.display());
    assert!(stdout.contains("Executed 2 tests") || stderr.contains("Executed 2 tests"), "{stdout}\n{stderr}");
}
