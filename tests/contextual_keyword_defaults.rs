#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn test_helpers_keep_their_own_contract_beside_same_named_app_helpers() {
    let run = emit_and_run::real_blog()
        .write("app/helpers/native_label_helper.rb", r#"module NativeLabelHelper
  def native_label(style: :app, suffix: "app")
    [style, suffix]
  end
end
"#)
        .write("test/models/native_label_test.rb", r#"require "test_helper"
class NativeLabelTest < ActiveSupport::TestCase
  def native_label(style: :test, suffix: "test")
    [style, suffix]
  end
  setup do
    @setup_label = native_label(suffix: "setup")
  end
  test "test helper keywords" do
    assert_equal [:test, "setup"], @setup_label
    assert_equal [:test, "body"], native_label(suffix: "body")
  end
end
"#)
        .run_test("test/models/native_label_test.rb");
    run.assert_passes();
}

#[test]
fn a_cross_helper_keyword_packet_keeps_the_exported_contract() {
    let run = emit_and_run::real_blog()
        .write("app/helpers/native_target_helper.rb", r#"module NativeTargetHelper
  def native_target(style: :time, **attributes)
    [style, attributes]
  end
end
"#)
        .write("app/helpers/native_source_helper.rb", r#"module NativeSourceHelper
  def native_source(**attributes)
    native_target(**attributes)
  end
end
"#)
        .run_ruby("require_relative 'app/models/native_target_helper'\nrequire_relative 'app/models/native_source_helper'\np NativeSourceHelper.native_source(class: 'stamp')\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[:time, {class: \"stamp\"}]\n");
}

#[test]
fn native_form_wrappers_keep_omitted_and_literal_keyword_options() {
    let run = emit_and_run::real_blog()
        .write("app/helpers/native_form_helper.rb", r#"module NativeFormHelper
  def native_article_form(article, **options, &)
    form_with model: article, url: articles_path, class: "native-wrapper-default", **options, &
  end
end
"#)
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @native_article = Article.new\n")
        .edit("app/views/articles/index.html.erb", "<h1", r#"<%= native_article_form @native_article do |form| %><%= form.text_field :title %><% end %>
<%= native_article_form @native_article, class: "native-wrapper-custom" do |form| %><%= form.text_field :title %><% end %>
<h1"#)
        .write("test/controllers/native_keyword_forms_controller_test.rb", r#"require "test_helper"
class NativeKeywordFormsControllerTest < ActionDispatch::IntegrationTest
  test "keyword wrapper forms render" do
    get articles_url
    assert_response :success
    assert_includes response.body, 'class="native-wrapper-default"'
    assert_includes response.body, 'class="native-wrapper-custom"'
    assert_includes response.body, 'name="article[title]"'
  end
end
"#)
        .run_test("test/controllers/native_keyword_forms_controller_test.rb");
    run.assert_passes();
}

#[test]
fn framework_keyword_packets_keep_their_existing_lowering() {
    let run = emit_and_run::real_blog()
        .write("app/helpers/native_packet_helper.rb", r#"module NativePacketHelper
  def packet_tag(style: :time, **attributes)
    tag.time **attributes, datetime: "fixed", data: { local_time_target: style }
  end
end
"#)
        .run_ruby("require_relative 'app/models/native_packet_helper'\nputs NativePacketHelper.packet_tag(class: 'stamp')\n");
    run.assert_passes();
    assert_eq!(run.stdout, "<time class=\"stamp\" datetime=\"fixed\" data-local-time-target=\"time\"></time>\n");
}

const CLIENT: &str = r#"class KeywordClient
  def initialize
    @request_timeout = 17
    @calls = 0
  end

  def next_timeout
    @calls += 1
    @request_timeout + @calls
  end

  def request!(payload = nil, timeout: next_timeout, require_request: nil, **keyword_payload)
    [payload, timeout, require_request, keyword_payload, @calls]
  end

  def dependent(value, timeout: value, label: nil)
    [timeout, label]
  end

  def report
    [request!({"type" => "sync"}, require_request: "sync"),
     request!({"type" => "sync"}, timeout: nil, require_request: "sync"),
     dependent(23, label: "kept")]
  end
end
"#;

const TRANSCRIPT: &str = r#"class KeywordTranscript
  def item(record, type, payload = nil, id: nil, **fields)
    [record, type, payload, id, fields]
  end

  def claude_item(record, type, id: record["uuid"].to_s, **fields)
    [type, id, fields]
  end

  def report
    [item("record", "assistant", text: "hello"),
     item("record", "assistant", id: "explicit", record: "field"),
     item("record", "assistant"),
     claude_item({"uuid" => "uuid-1"}, "thinking", text: "thought")]
  end
end
"#;

#[test]
fn defaults_run_in_the_callee_only_when_omitted() {
    let run = emit_and_run::real_blog()
        .write("app/services/keyword_client.rb", CLIENT)
        .run_ruby("require_relative 'app/models/keyword_client'\np KeywordClient.new.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[[{\"type\" => \"sync\"}, 18, \"sync\", {}, 1], [{\"type\" => \"sync\"}, nil, \"sync\", {}, 1], [23, \"kept\"]]\n");
}

#[test]
fn unmatched_keywords_bind_to_rest_beside_optional_keywords() {
    let run = emit_and_run::real_blog()
        .write("app/services/keyword_transcript.rb", TRANSCRIPT)
        .run_ruby("require_relative 'app/models/keyword_transcript'\np KeywordTranscript.new.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[[\"record\", \"assistant\", nil, nil, {text: \"hello\"}], [\"record\", \"assistant\", nil, \"explicit\", {record: \"field\"}], [\"record\", \"assistant\", nil, nil, {}], [\"thinking\", \"uuid-1\", {text: \"thought\"}]]\n");
}

#[test]
fn a_dynamic_keyword_splat_binds_optional_keywords() {
    let run = emit_and_run::real_blog()
        .write("app/services/keyword_status.rb", r#"class KeywordStatus
  def self.started(attempt: nil, token: nil, phase: nil)
    [attempt, token, phase]
  end

  def self.report
    attributes = {phase: "checkout", token: "token-1"}
    started(**attributes)
  end
end
"#)
        .run_ruby("require_relative 'app/models/keyword_status'\np KeywordStatus.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[nil, \"token-1\", \"checkout\"]\n");
}
