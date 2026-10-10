use super::emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn request_put_predicate_runs_natively() {
    let run = emit_and_run::real_blog()
        // Runtime tree shaking drops unused framework methods. The
        // native consumer is appended after shaking, so add an app-level
        // caller to keep this predicate in the emitted runtime.
        .write(
            "app/controllers/request_put_probe_controller.rb",
            "class RequestPutProbeController < ApplicationController\n  def probe\n    request.put?\n  end\nend\n",
        )
        .run_spinel(
            r#"request = ActionDispatch::Request.new
raise "GET request matched PUT" if request.put?
request.request_method = "PUT"
raise "PUT request did not match" unless request.put?
raise "PUT request matched GET" if request.get?
request.request_method = "put"
raise "lowercase method matched PUT" if request.put?
puts "ActionDispatch::Request#put? native passed"
"#,
        );
    run.assert_passes();
    assert!(run
        .stdout
        .contains("ActionDispatch::Request#put? native passed"));
}
