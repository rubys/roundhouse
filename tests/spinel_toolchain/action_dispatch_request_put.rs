use super::emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn request_put_predicate_runs_natively() {
    let run = emit_and_run::real_blog()
        // Runtime tree shaking sees application/test source before the
        // native consumer is appended, so keep this otherwise-unused
        // request predicate reachable in the emitted framework runtime.
        .write(
            "test/request_put_predicate_probe.rb",
            "request = ActionDispatch::Request.new\nrequest.put?\n",
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
