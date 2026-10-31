use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ASSOCIATION: &str = "has_many :received_messages, class_name: \"Message\", foreign_key: :to_pty_session_id";

fn files(body: &str, association: &str) -> Vec<(&'static str, String)> {
    vec![
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into()),
        ("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table :pty_sessions do |t|
    t.string :name
  end
  create_table :messages do |t|
    t.integer :to_pty_session_id
    t.integer :in_reply_to_id
    t.string :body
  end
end
"#.into()),
        ("app/models/pty_session.rb", format!("class PtySession < ApplicationRecord\n  {association}\nend\n")),
        ("app/models/message.rb", "class Message < ApplicationRecord\nend\n".into()),
        ("config/routes.rb", "Rails.application.routes.draw do\n  post \"/messages\", to: \"messages#create\"\nend\n".into()),
        ("app/controllers/messages_controller.rb", format!(r#"class MessagesController < ActionController::API
  before_action :set_pty_session

  def create
{body}
  end

  private

  def set_pty_session
    @pty_session = PtySession.find_by(id: params[:id])
    head :not_found unless @pty_session
  end

  def message_params
    params.require(:message).permit(:body, :in_reply_to_id)
  end

  def other_params
    params.require(:other).permit(:body)
  end

  def switch_attributes
    @calls += 1
    @pty_session = PtySession.find(params[:replacement_id])
    {{ body: "switched" }}
  end

  def uncertain_attributes
    params[:choose] ? message_params : "not attributes"
  end
end
"#)),
    ]
}

fn emitted(body: &str, association: &str) -> (String, String) {
    let tree: HashMap<PathBuf, Vec<u8>> = files(body, association).into_iter()
        .map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(diagnostics.is_empty(), "unexpected lowering diagnostics: {diagnostics:#?}");
    let files = ruby::emit_lowered_controllers(&app);
    let content = |suffix: &str| files.iter()
        .find(|file| file.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("missing {suffix}")).content.clone();
    (content("controllers/messages_controller.rb"), content("models/message_params.rb"))
}

#[test]
fn local_and_ivar_attribute_hashes_and_local_keywords_build_the_declared_model() {
    for source in [
        "    attributes = { body: \"hash\" }\n    message = @pty_session.received_messages.new(attributes)",
        "    attributes = { body: \"hash\" }\n    @message = @pty_session.received_messages.new(attributes)",
        "    attributes = { body: \"hash\" }\n    copied = attributes\n    message = @pty_session.received_messages.new(copied)",
        "    message = @pty_session.received_messages.new(switch_attributes)",
        "    message = @pty_session.received_messages.new(switch_attributes.merge(body: \"merged\"))",
        "    message = @pty_session.received_messages.new(body: \"literal\")",
    ] {
        let (controller, _) = emitted(source, ASSOCIATION);
        assert!(!controller.contains("received_messages.build("), "{controller}");
        assert!(!controller.contains("received_messages.new("), "{controller}");
        assert!(controller.contains("Message.new("), "{controller}");
        assert!(controller.contains("to_pty_session_id:"), "{controller}");
        if !source.contains("@message =") {
            assert!(!controller.contains("@message ="), "local destination became an ivar:\n{controller}");
        }
    }
}

#[test]
fn aliases_and_except_use_the_params_factory_and_request_a_copying_filter() {
    let (controller, params) = emitted(r#"
    attributes = message_params
    alias_attributes = attributes
    message = @pty_session.received_messages.new(alias_attributes.except(:in_reply_to_id))
    filtered_attributes = alias_attributes.except(:in_reply_to_id)
    copied_attributes = filtered_attributes
    second = @pty_session.received_messages.new(copied_attributes)
"#, ASSOCIATION);
    assert!(controller.contains("Message.from_params(alias_attributes.except(:in_reply_to_id))"), "{controller}");
    assert!(controller.contains("Message.from_params(copied_attributes)"), "{controller}");
    assert!(controller.contains(".to_pty_session_id = rh_build_owner_"), "{controller}");
    assert!(!controller.contains("received_messages.build("), "{controller}");
    assert!(params.contains("def except(*keys)"), "{params}");
    assert!(params.contains("result.in_reply_to_id_provided = false"), "{params}");
    assert!(!params.contains("@in_reply_to_id_provided = false if"), "{params}");
}

#[test]
fn except_with_multiple_keys_matches_rails_without_mutating_the_original() {
    let body = r#"
    attributes = message_params
    alias_attributes = attributes
    filtered = alias_attributes.except(:in_reply_to_id, :body)
    @original = attributes
    @filtered = filtered
    @unchanged = attributes.except
    @repeated = attributes.except(:unknown, :body, :body)
    string_filtered = alias_attributes.except("in_reply_to_id")
    mixed_filtered = alias_attributes.except("in_reply_to_id", :body)
    @string = string_filtered
    @mixed = mixed_filtered
    @mixed_repeated = attributes.except("unknown", "body", :body)
    message = @pty_session.received_messages.new(filtered)
    message.save!
    string_message = @pty_session.received_messages.new(string_filtered)
    string_message.save!
    mixed_message = @pty_session.received_messages.new(mixed_filtered)
    mixed_message.save!
    head :ok
"#;
    let mut overlay = emit_and_run::empty_app();
    for (path, source) in files(body, ASSOCIATION) { overlay = overlay.write(path, &source); }
    let run = overlay.run_ruby(r#"
require "json"
require_relative "app/controllers/messages_controller"
owner = PtySession.create!(name: "owner")
controller = MessagesController.new
controller.instance_variable_set(:@params, {
  "id" => owner.id.to_s, "message" => {"body" => "kept", "in_reply_to_id" => "33"}
})
controller.create
row, string_row, mixed_row = Message.all.to_a
raise "filtered build" unless row.body.nil? && row.in_reply_to_id.nil? && row.to_pty_session_id == owner.id
raise "String-filtered build" unless string_row.body == "kept" && string_row.in_reply_to_id.nil? && string_row.to_pty_session_id == owner.id
raise "mixed-filtered build" unless mixed_row.body.nil? && mixed_row.in_reply_to_id.nil? && mixed_row.to_pty_session_id == owner.id
def provided_values(params)
  values = {}
  values["body"] = params.body if params.body_provided
  values["in_reply_to_id"] = params.in_reply_to_id if params.in_reply_to_id_provided
  values
end
puts JSON.generate([:@original, :@filtered, :@unchanged, :@repeated, :@string, :@mixed, :@mixed_repeated].map do |name|
  provided_values(controller.instance_variable_get(name))
end)
"#);
    run.assert_passes();
    assert_eq!(run.stdout.trim(), r#"[{"body":"kept","in_reply_to_id":"33"},{},{"body":"kept","in_reply_to_id":"33"},{"in_reply_to_id":"33"},{"body":"kept"},{},{"in_reply_to_id":"33"}]"#);
}

#[test]
fn a_reassignment_or_conditional_binding_is_not_a_params_proof() {
    for (source, proven_hash) in [
        ("    attributes = message_params\n    attributes = { body: \"hash\" }\n    message = @pty_session.received_messages.new(attributes)", true),
        ("    if params[:choose]\n      attributes = message_params\n    end\n    message = @pty_session.received_messages.new(attributes)", false),
        ("    attributes = message_params\n    [{ body: \"block\" }].each do |attributes|\n      message = @pty_session.received_messages.new(attributes)\n    end", false),
        ("    attributes = message_params\n    [1, 2].each do |i|\n      message = @pty_session.received_messages.new(attributes)\n      attributes = { body: \"next iteration\" }\n    end", false),
        ("    message = @pty_session.received_messages.new(uncertain_attributes)", false),
    ] {
        let (controller, _) = emitted(source, ASSOCIATION);
        assert!(!controller.contains("Message.from_params("), "false params proof:\n{controller}");
        if !proven_hash {
            assert!(!controller.contains(".merge(to_pty_session_id:"), "unproven Hash:\n{controller}");
            assert!(!controller.contains("rh_build_owner_"), "unproven build capture:\n{controller}");
        }
    }
}

#[test]
fn a_declared_row_changing_scope_still_declines_local_build_expansion() {
    let scoped = "has_many :received_messages, -> { where(body: \"scoped\") }, class_name: \"Message\", foreign_key: :to_pty_session_id";
    let (controller, _) = emitted("    attributes = message_params\n    message = @pty_session.received_messages.new(attributes)", scoped);
    assert!(controller.contains("received_messages."), "{controller}");
    assert!(!controller.contains("Message.from_params("), "{controller}");
    assert!(!controller.contains("rh_build_owner_"), "{controller}");
}

#[test]
fn an_alias_of_another_resources_params_does_not_select_a_missing_factory() {
    let (controller, _) = emitted(r#"
    attributes = message_params
    attributes = other_params
    message = @pty_session.received_messages.new(attributes)
"#, ASSOCIATION);
    assert!(!controller.contains("Message.from_params("), "{controller}");
    assert!(controller.contains("received_messages.build("), "{controller}");
}

#[test]
fn emitted_builds_preserve_inputs_owner_evaluation_and_assignment_results() {
    let body = r#"
    rh_build_owner_0 = "written local"
    rh_build_record_0 = "written record"
    attributes = { body: "hash", to_pty_session_id: 999 }
    message = @pty_session.received_messages.new(attributes)
    message.save!
    @attributes = attributes
    @message = @pty_session.received_messages.new(attributes)
    @message.save!
    literal = @pty_session.received_messages.new(body: "literal")
    literal.save!
    attributes = message_params
    alias_attributes = attributes
    filtered = @pty_session.received_messages.new(alias_attributes.except(:in_reply_to_id))
    filtered.save!
    @original = attributes
    @calls = 0
    switched = @pty_session.received_messages.new(switch_attributes)
    switched.save!
    execute = false
    execute && (skipped = @pty_session.received_messages.new(switch_attributes))
    result = (assigned = @pty_session.received_messages.new(message_params))
    @result = result
    @assigned = assigned
    @written_owner = rh_build_owner_0
    @written_record = rh_build_record_0
    head :ok
"#;
    let mut overlay = emit_and_run::empty_app();
    for (path, source) in files(body, ASSOCIATION) { overlay = overlay.write(path, &source); }
    overlay.run_ruby(r#"
require_relative "app/controllers/messages_controller"
owner = PtySession.create!(name: "original")
replacement = PtySession.create!(name: "replacement")
controller = MessagesController.new
controller.instance_variable_set(:@params, {
  "id" => owner.id.to_s, "replacement_id" => replacement.id.to_s,
  "message" => { "body" => "filtered", "in_reply_to_id" => "33" }
})
controller.create
rows = Message.all.to_a.map { |row| [row.body, row.to_pty_session_id, row.in_reply_to_id] }
expected = [["hash", owner.id, nil], ["hash", owner.id, nil], ["literal", owner.id, nil],
            ["filtered", owner.id, nil], ["switched", owner.id, nil]]
raise "persisted rows: #{rows.inspect}" unless rows == expected
raise "mutated hash" unless controller.instance_variable_get(:@attributes) == {body: "hash", to_pty_session_id: 999}
original = controller.instance_variable_get(:@original)
raise "mutated params" unless original.in_reply_to_id_provided && original.in_reply_to_id == "33"
raise "repeated attributes" unless controller.instance_variable_get(:@calls) == 1
result = controller.instance_variable_get(:@result)
assigned = controller.instance_variable_get(:@assigned)
raise "assignment result" unless result.is_a?(Message) && result.equal?(assigned) && result.to_pty_session_id == replacement.id
raise "local scope" if controller.instance_variable_defined?(:@filtered)
raise "captured local" unless controller.instance_variable_get(:@written_owner) == "written local"
raise "captured record" unless controller.instance_variable_get(:@written_record) == "written record"
missing = MessagesController.new
missing.instance_variable_set(:@params, { "id" => "99999" })
missing.create
raise "missing owner guard" unless missing.status == 404 && Message.count == 5
puts "local association builds: rows, inputs, owner, evaluation, assignment"
"#).assert_passes();
}
