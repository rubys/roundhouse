use super::emit_and_run;

#[test]
fn request_put_predicate_runs_in_emitted_cruby() {
    let run = emit_and_run::real_blog().run_ruby(
        r#"request = ActionDispatch::Request.new("REQUEST_METHOD" => "GET")
raise "GET request matched PUT" if request.put?
request = ActionDispatch::Request.new("REQUEST_METHOD" => "PUT")
raise "PUT request did not match" unless request.put?
raise "PUT request matched GET" if request.get?
request = ActionDispatch::Request.new("REQUEST_METHOD" => "put")
raise "lowercase method matched PUT" if request.put?
puts "ActionDispatch::Request#put? passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("ActionDispatch::Request#put? passed"));
}
