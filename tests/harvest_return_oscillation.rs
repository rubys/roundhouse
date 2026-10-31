//! Fixpoint rounds must not thrash the harvest table — and must not
//! launder a genuinely gradual return into a concrete one either.
//!
//! A `config` / `Configuration.load!` cycle can alternate
//! `Configuration ↔ Configuration|Untyped` every fixpoint round and
//! exhaust `FIXPOINT_CAP`. Stabilizing returns that differ only by
//! top-level `Untyped`/`Var` arms lets the signature converge.
//!
//! A different shape hits the same stabilizer: a parameter unbound on
//! the first round (a pending `Var`) and seeded `untyped` by a later
//! round's call site, with a concrete arm (`Sym`) present both times.
//! That `untyped` is not fixpoint noise — it is the only honest answer
//! — so it must survive, not be dropped for the concrete arm alone.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

const CIRCULAR: &str = r#"
module Probe
  def self.config
    @config ||= Configuration.load!
  end

  class Configuration
    def self.load!
      if config_exists?
        Probe.config.apply_upgrades
      else
        new
      end
    end

    def self.config_exists?
      true
    end

    def apply_upgrades
      self
    end
  end
end

class Client
  def boot
    Probe.config.apply_upgrades
  end
end
"#;

// `locale_for`'s parameter has no call site on the first pass over
// `lib/locale.rb` (`resolve` is defined after it), so round one types
// `x` from an unbound local — a pending `Var` — and `x&.dig("lang")`
// joins it with `Sym`. Once `resolve`'s body is walked, its call site
// seeds `x` as `untyped` (from `JSON.parse`'s unknown return), and
// round two's body type is `Sym | Untyped`. Both rounds share the
// `Sym` core, so stabilize used to drop the `untyped` arm outright —
// declaring `locale_for` `-> Symbol`, though it answers whatever
// `JSON.parse` read at the key. #964; #395 is the same family.
const LOCALE_PARAM: &str = r#"
module Locale
  def self.locale_for(x)
    x&.dig("lang") || :en
  end

  def self.resolve(raw)
    locale_for(JSON.parse(raw))
  end
end
"#;

#[test]
fn param_seeded_untyped_after_a_pending_var_keeps_the_gradual_arm() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/locale.rb"), LOCALE_PARAM.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rbs.contains("def self.locale_for: (untyped x) -> untyped"),
        "expected locale_for to stay declared -> untyped (it answers a JSON value, \
         not always a Symbol), got:\n{rbs}"
    );
}

#[test]
fn circular_config_load_emits_configuration_not_gradual_churn() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/probe.rb"), CIRCULAR.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    // RBS collapses `T | untyped` to bare `untyped`, so checking for the
    // substring "Configuration | untyped" never fails. Pin the settled
    // signatures themselves — without stabilize, `config` ends as
    // `-> untyped` while the class name still matches a naive search.
    assert!(
        rbs.contains("def self.config: () -> Probe::Configuration"),
        "expected Probe.config to settle on Probe::Configuration, got:\n{rbs}"
    );
    assert!(
        rbs.contains("def self.load!: () -> Probe::Configuration"),
        "expected Configuration.load! to settle on Probe::Configuration, got:\n{rbs}"
    );
}

#[test]
fn pending_presence_chain_with_a_nilable_name_still_emits_string_or_nil() {
    let mut tree = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    tree.insert(PathBuf::from("lib/a_user.rb"), br#"
class User
  def name
    "Ada"
  end
end
"#.to_vec());
    tree.insert(PathBuf::from("lib/room_names.rb"), br#"
module RoomNames
  def self.room_display_name(title)
    for_user = [User.new, nil].first
    title.presence || for_user&.name
  end

  def self.for_room
    room_display_name("Room")
  end
end
"#.to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app).into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content).collect::<Vec<_>>().join("\n");
    assert!(rbs.contains("def self.room_display_name: (String title) -> String?"),
            "pending presence must not leave a gradual return, got:\n{rbs}");
}

#[test]
fn gradual_json_value_forwarded_through_a_pending_stub_stays_gradual() {
    let mut tree = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    tree.insert(PathBuf::from("lib/wrapped_locale.rb"), br#"
module WrappedLocale
  def self.locale_for(x)
    x&.dig("lang") || :en
  end

  def self.forward(x)
    x
  end

  def self.resolve(raw)
    locale_for(forward(JSON.parse(raw)))
  end
end
"#.to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app).into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content).collect::<Vec<_>>().join("\n");
    assert!(rbs.contains("def self.locale_for: (untyped x) -> untyped"),
            "a forwarded JSON value is still gradual, got:\n{rbs}");
    assert!(rbs.contains("def self.resolve: (untyped raw) -> untyped"),
            "forwarding a JSON value must not declare resolve as Symbol, got:\n{rbs}");
}

fn declarations_for(source: &str) -> (roundhouse::App, String) {
    declarations_with_caller(source, None)
}

fn declarations_with_caller(source: &str, caller: Option<&str>) -> (roundhouse::App, String) {
    let mut tree = HashMap::from([
        (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec()),
        (PathBuf::from("lib/probe.rb"), source.as_bytes().to_vec()),
    ]);
    if let Some(caller) = caller {
        tree.insert(PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw { get '/probe', to: 'probes#show' }".to_vec());
        tree.insert(PathBuf::from("app/controllers/probes_controller.rb"), format!(
            "class ProbesController < ActionController::Base\n def show\n {caller}\n head :ok\n end\nend\n").into_bytes());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = emit_library(&app).into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content).collect::<Vec<_>>().join("\n");
    (app, rbs)
}

#[test]
fn room_return_and_broadcast_parameter_admit_the_runtime_object() {
    let (_, rbs) = declarations_for(r#"class Rooms
  Room = Struct.new(:lock, :members)

  def self.room(id)
    state = Rails.application.room_state
    state[:lock].synchronize { state[:rooms][id] ||= Room.new(Mutex.new, {}) }
  end

  def self.join(id, socket)
    room = room(id)
    room.lock.synchronize do
      return if socket.closed?
      return :full if room.members.length >= 100
      broadcast(room)
    end
    room
  end

  def self.broadcast(room)
    room.members.each_key { |socket| socket.send("presence") }
  end

  def self.open(id, socket)
    result = join(id, socket)
    return if result == :full
    broadcast(result)
  end
end
"#);
    for name in ["join", "room"] {
        let line = rbs.lines().find(|line| line.contains(&format!("def self.{name}:"))).unwrap();
        assert!(line.ends_with("-> untyped") || line.contains("-> Rooms::Room"), "{rbs}");
    }
    assert!(rbs.contains("def self.broadcast: (untyped room)"), "{rbs}");
}

#[test]
fn gradual_arguments_stay_unknown_bare_or_beside_a_fallback() {
    for call in [r#"take(JSON.parse("42") || :fallback)"#,
                 r#"take(:fallback); take(JSON.parse("42"))"#,
                 r#"take(JSON.parse("42")); take(:fallback)"#] {
        let source = format!("module Probe\n def self.take(value); value; end\n def self.run; {call}; end\nend\n");
        let (_, rbs) = declarations_for(&source);
        assert!(rbs.contains("def self.take: (untyped value) -> untyped"), "{call}: {rbs}");
    }
}

#[test]
fn known_block_unwinds_retain_invalid_call_errors() {
    for body in [
        "JSON.parse(raw).tap { explode } rescue \"ok\"",
        "JSON.parse(raw).tap { p = Kernel.proc { return \"ok\" }; alias_proc = p; alias_proc[] }",
        "JSON.parse(raw).tap { begin; Kernel.raise(\"x\", \"y\"); rescue RuntimeError; nil; end } rescue \"ok\"",
    ] {
        let source = format!("module Probe\n def self.value(raw); {body}; end\n def self.explode; raise \"x\"; end\n def self.run; value(\"42\").no_such_method; end\nend\n");
        let (app, rbs) = declarations_with_caller(&source, Some(r#"Probe.value("42").no_such_method"#));
        assert!(rbs.contains("def self.value: (String raw) -> String"), "{body}: {rbs}");
        let diagnostics = roundhouse::analyze::diagnose(&app);
        assert!(diagnostics.iter().any(|d| matches!(&d.kind,
            roundhouse::diagnostic::DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "no_such_method")),
            "{body}: {diagnostics:?}");
    }
}

#[test]
fn unknown_bearing_unions_report_the_gradual_boundary() {
    let (app, _) = declarations_with_caller("module Probe\n def self.value; JSON.parse(\"42\") || :fallback; end\nend\n",
        Some("Probe.value.no_such_method"));
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(diagnostics.iter().any(|d| matches!(&d.kind,
        roundhouse::diagnostic::DiagnosticKind::GradualUntyped { .. })
        && app.sources[d.span.file.0 as usize - 1].text[d.span.start as usize..].starts_with("Probe.value.no_such_method")),
        "the unknown-bearing call must report its own boundary: {diagnostics:?}");
}

#[test]
fn computed_unknown_results_are_not_promises_of_the_fallback_type() {
    let (_, rbs) = declarations_for(r#"module Probe
  def self.value
    [JSON.parse("42")].first || :fallback
  end
end
"#);
    assert!(rbs.contains("def self.value: () -> untyped"), "{rbs}");
}

#[test]
fn customized_json_values_do_not_certify_a_core_yielding_protocol() {
    for parser in [
        "JSON.load(raw, nil, create_additions: true)",
        "JSON.parse(raw, object_class: ParserValue)",
    ] {
        let source = format!(r#"class ParserValue < Hash
  def self.json_create(value); new; end
  def tap; 42; end
end
module Probe
  def self.value(raw)
    {parser}.tap {{ raise "not yielded" }} rescue :fallback
  end
end
"#);
        let (_, rbs) = declarations_for(&source);
        assert!(rbs.contains("def self.value: (untyped raw) -> untyped"), "{parser}: {rbs}");
    }
}
