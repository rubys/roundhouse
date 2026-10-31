#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const TRANSCRIPT: &str = r#"class Transcript
  def collect(**fields)
    fields
  end

  def positional_hash
    collect({text: "positional"})
  end

  def self.fields(**fields)
    fields
  end

  def self.item(record, type, payload = nil, id: nil, **fields)
    [record, type, payload, id, fields]
  end

  def self.claude(record, type, id: record["uuid"].to_s, **fields)
    [type, id, fields]
  end

  def self.example
    item("record", "assistant", text: "hello")
  end

  def self.explicit
    item("record", "assistant", "payload", text: "hello", id: "chosen")
  end

  def self.positional_names
    item("record", "assistant", record: "extra", payload: "extra payload", fields: "extra fields")
  end

  def self.default_id
    claude({"uuid" => "event-7"}, "error", text: "failed")
  end
end
"#;

const STATUS: &str = r#"class Status
  def self.started(attempt: nil, token: nil, phase: nil)
    [attempt, token, phase]
  end

  def self.example
    attributes = {token: "secret", attempt: "attempt-2"}
    started(**attributes)
  end

  def self.partial
    attributes = {phase: "checkout"}
    started(**attributes)
  end

  def self.mixed
    attributes = {token: "secret"}
    started(attempt: "attempt-2", **attributes)
  end

  def self.empty
    attributes = {}
    started(**attributes)
  end

  def self.packet
    @calls = @calls + 1
    {token: "secret"}
  end

  def self.once
    @calls = 0
    result = started(**packet)
    [@calls, result]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/transcript.rb", TRANSCRIPT)
        .write("app/models/status.rb", STATUS)
}

#[test]
fn unknown_keywords_bind_to_rest_without_filling_optional_positionals() {
    app().run_ruby(r#"
raise Transcript.example.inspect unless Transcript.example == ["record", "assistant", nil, nil, {text: "hello"}]
raise Transcript.explicit.inspect unless Transcript.explicit == ["record", "assistant", "payload", "chosen", {text: "hello"}]
raise Transcript.positional_names.inspect unless Transcript.positional_names == ["record", "assistant", nil, nil, {record: "extra", payload: "extra payload", fields: "extra fields"}]
raise Transcript.default_id.inspect unless Transcript.default_id == ["error", "event-7", {text: "failed"}]
puts "keyword rest and contextual default passed"
"#).assert_passes();
}

#[test]
fn dynamic_keyword_splats_bind_by_name_and_evaluate_once() {
    app().run_ruby(r#"
raise Status.example.inspect unless Status.example == ["attempt-2", "secret", nil]
raise Status.partial.inspect unless Status.partial == [nil, nil, "checkout"]
raise Status.mixed.inspect unless Status.mixed == ["attempt-2", "secret", nil]
raise Status.empty.inspect unless Status.empty == [nil, nil, nil]
raise Status.once.inspect unless Status.once == [1, [nil, "secret", nil]]
puts "dynamic keywords passed"
"#).assert_passes();
}

#[test]
fn unknown_keys_and_a_positional_hash_still_raise() {
    app().run_ruby(r#"
begin
  Status.started(**{unknown: "bad"})
  raise "unknown keyword accepted"
rescue ArgumentError
end
begin
  Status.started({attempt: "positional"})
  raise "positional Hash accepted as keywords"
rescue ArgumentError
end
begin
  Transcript.fields({text: "positional"})
  raise "positional Hash accepted as keyword rest"
rescue ArgumentError
end
begin
  Transcript.new.positional_hash
  raise "positional Hash restored inside an instance method"
rescue ArgumentError
end
begin
  Transcript.new.collect({text: "positional"})
  raise "typed receiver converted a positional Hash to keyword rest"
rescue ArgumentError
end
puts "invalid keyword calls rejected"
"#).assert_passes();
}
