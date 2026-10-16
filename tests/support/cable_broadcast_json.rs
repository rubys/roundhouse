//! One `ActionCable.server.broadcast` encoding contract (#619) shared by
//! the interpreted (CRuby overlay) and native (spinel) lanes.
//!
//! The expected lines are what actioncable 8.1.4 records for the same
//! payloads (`ActionCable.server.pubsub.broadcasts`, the test adapter):
//! ActiveSupport::JSON writes every value as JSON (a String, nil, a
//! Float, a Symbol, a nested Hash or Array), escapes `<`, `>` and `&`
//! inside strings, and keeps other characters as they are.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
}

pub const SCRIPT: &str = r#"
ActionCable.server.broadcast("s", { "room_id" => 1 })
ActionCable.server.broadcast("s", { "o" => "G1s/MTA0OQ==" })
ActionCable.server.broadcast("s", { "say" => "<b>Done</b> & \"co\"\n" })
ActionCable.server.broadcast("s", { "end" => true, "nothing" => nil, "f" => 1.5 })
ActionCable.server.broadcast("s", { "user" => { "id" => 7, "name" => "Zażółć" } })
ActionCable.server.broadcast("s", { "list" => [1, "a", nil, false] })
ActionCable.server.broadcast("s", { action: :start, "sym" => :x })
ActionCable.server.pubsub.broadcasts("s").each { |b| puts b }
"#;

pub const EXPECTED: &str = r#"{"room_id":1}
{"o":"G1s/MTA0OQ=="}
{"say":"\u003cb\u003eDone\u003c/b\u003e \u0026 \"co\"\n"}
{"end":true,"nothing":null,"f":1.5}
{"user":{"id":7,"name":"Zażółć"}}
{"list":[1,"a",null,false]}
{"action":"start","sym":"x"}
"#;
