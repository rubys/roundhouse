use super::response_content_type_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn response_content_types_run_natively() {
    let run = contract::app().run_spinel(r#"controller = ContentProbesController.new
controller.process_action(:json)
raise "json content type" unless controller.content_type == "application/json; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:plain)
raise "plain content type" unless controller.content_type == "text/plain; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:html)
raise "html content type" unless controller.content_type == "text/html; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:vendor_json)
raise "vendor_json content type" unless controller.content_type == "application/vnd.api+json; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:json_charset)
raise "json_charset content type" unless controller.content_type == "application/json; charset=iso-8859-1"
controller = ContentProbesController.new
controller.process_action(:head_ok)
raise "head content type" unless controller.content_type == "text/html"
controller = ContentProbesController.new
controller.process_action(:head_csv)
raise "head_csv content type" unless controller.content_type == "text/csv"
controller = ContentProbesController.new
controller.process_action(:send_csv)
raise "send_csv content type" unless controller.content_type == "text/csv"
controller = ContentProbesController.new
controller.process_action(:send_binary)
raise "send_binary content type" unless controller.content_type == "application/octet-stream"
controller = ContentProbesController.new
controller.process_action(:redirect)
raise "redirect content type" unless controller.content_type == "text/html; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:plain_csv)
raise "plain_csv content type" unless controller.content_type == "text/csv; charset=utf-8"
controller = ContentProbesController.new
controller.process_action(:plain_charset)
raise "plain_charset content type" unless controller.content_type == "text/plain; charset=iso-8859-1"
controller = ContentProbesController.new
controller.process_action(:head_json)
raise "head_json content type" unless controller.content_type == "application/json"
controller = ContentProbesController.new
controller.process_action(:head_no_content)
raise "head_no_content content type" unless controller.content_type == ""
controller = ContentProbesController.new
controller.process_action(:send_json)
raise "send_json content type" unless controller.content_type == "application/json"
controller = ContentProbesController.new
controller.process_action(:inherited_charset)
raise "inherited_charset content type" unless controller.content_type == "text/csv; charset=iso-8859-1"
puts "response content types passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("response content types passed"));
}
