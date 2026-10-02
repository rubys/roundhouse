//! Generated surfaces for the first supported declaration families.
//! These rules are attribution evidence, never typed methods.
//!
//! `aasm` and `state_machines` name their methods after the author's
//! states and events (`event :publish` → `publish!`, `may_publish?`), so
//! no fixed list in `gems::SURFACES` can hold them. The rules below were
//! recorded against aasm 6.0 and state_machines-activerecord 0.200 by
//! diffing each declaring class's methods against a bare model's.

use std::collections::BTreeSet;

use ruby_prism::{CallNode, Node};

use super::{Surface, literal_arguments, name_of, option, positional, statements};

/// `state :draft` → `draft?` and the `draft` scope; `event :publish` →
/// `publish`, `publish!`, `publish_without_validation!`, `may_publish?`.
/// Under `namespace: :rev` states read `rev_draft?` (the scope keeps
/// both spellings) and events `publish_rev`.
pub(super) fn aasm(call: &CallNode<'_>, body: Option<Node<'_>>) -> Vec<Surface> {
    if !literal_arguments(call) {
        return Vec::new();
    }
    let namespace = option(call, "namespace");
    if namespace
        .as_ref()
        .is_some_and(|value| name_of(value).is_none() && value.as_nil_node().is_none())
    {
        return Vec::new();
    }
    let namespace = namespace.as_ref().and_then(name_of);
    if option(call, "create_scopes")
        .is_some_and(|value| value.as_true_node().is_none() && value.as_false_node().is_none())
    {
        return Vec::new();
    }
    let scopes = option(call, "create_scopes").map_or(true, |v| v.as_false_node().is_none());
    let (mut states, mut events) = (Vec::new(), Vec::new());
    for statement in statements(body) {
        let Some(inner) = statement.as_call_node().filter(|c| c.receiver().is_none()) else {
            return Vec::new();
        };
        if !literal_arguments(&inner) {
            return Vec::new();
        }
        match inner.name().as_slice() {
            b"state" if inner.block().is_none() => {
                states.extend(positional(&inner).iter().filter_map(name_of))
            }
            b"event" => {
                // The event DSL body executes before naming its methods.
                // Admit literal transition registrations only; unknown
                // execution can change the machine's configuration.
                if let Some(block) = inner.block() {
                    let Some(block) = block.as_block_node() else {
                        return Vec::new();
                    };
                    if let Some(body) = block.body() {
                        let Some(body) = body.as_statements_node() else {
                            return Vec::new();
                        };
                        for statement in body.body().iter() {
                            let Some(transition) = statement.as_call_node() else {
                                return Vec::new();
                            };
                            if transition.receiver().is_some()
                                || transition.name().as_slice() != b"transitions"
                                || transition.block().is_some()
                                || !literal_arguments(&transition)
                            {
                                return Vec::new();
                            }
                        }
                    }
                }
                events.extend(positional(&inner).first().and_then(name_of));
            }
            // Unmodeled executable statements can change the naming
            // configuration. Do not simulate mutation/order here.
            _ => return Vec::new(),
        }
    }
    let mut names = BTreeSet::new();
    let mut scope_names = BTreeSet::new();
    for state in &states {
        match &namespace {
            Some(ns) => {
                names.insert(format!("{ns}_{state}?"));
                if scopes {
                    scope_names.insert(format!("{ns}_{state}"));
                }
            }
            None => {
                names.insert(format!("{state}?"));
            }
        }
        if scopes {
            scope_names.insert(state.clone());
        }
    }
    for event in &events {
        let base = match &namespace {
            Some(ns) => format!("{event}_{ns}"),
            None => event.clone(),
        };
        names.insert(format!("{base}!"));
        names.insert(format!("{base}_without_validation!"));
        names.insert(format!("may_{base}?"));
        names.insert(base);
    }
    vec![
        Surface {
            names,
            ancestor: None,
            provider: None,
        },
        Surface {
            names: scope_names,
            ancestor: Some("ActiveRecord::Base"),
            provider: None,
        },
    ]
}

/// `state_machine :status` → the attribute's own surface (`status_name`,
/// `with_status`, …), each state's `open?`, and each event's `close`,
/// `close!`, `can_close?`, `close_transition`. States are also declared
/// by naming them in a transition or as `initial:`; under `namespace:
/// "alarm"` states read `alarm_open?` and events `close_alarm`.
pub(super) fn state_machine(call: &CallNode<'_>, body: Option<Node<'_>>) -> Vec<Surface> {
    if !literal_arguments(call) {
        return Vec::new();
    }
    // Explicit integration selection has a different scope/action profile.
    // Keep it outside this ActiveRecord-oriented naming subset.
    if option(call, "integration").is_some() {
        return Vec::new();
    }
    // An omitted name reopens the first visible machine, including an
    // inherited machine. Do not invent a default attribute/namespace.
    let Some(attr) = positional(call).first().and_then(name_of) else {
        return Vec::new();
    };
    let namespace = option(call, "namespace");
    if namespace
        .as_ref()
        .is_some_and(|value| name_of(value).is_none() && value.as_nil_node().is_none())
    {
        return Vec::new();
    }
    let namespace = namespace.as_ref().and_then(name_of);
    let mut states: BTreeSet<String> = option(call, "initial")
        .as_ref()
        .and_then(name_of)
        .into_iter()
        .collect();
    let mut events = BTreeSet::new();
    if let Some(body) = body {
        if !collect_machine(&body, &mut states, &mut events, false) {
            return Vec::new();
        }
    }
    let plural = match option(call, "plural") {
        Some(value) if value.as_nil_node().is_none() && value.as_false_node().is_none() => {
            let Some(plural) = name_of(&value) else {
                return Vec::new();
            };
            Some(plural)
        }
        _ if crate::naming::snake_case(&attr) == attr => {
            Some(crate::naming::pluralize_snake(&attr))
        }
        _ => None,
    };
    let mut names: BTreeSet<String> = [
        format!("{attr}?"),
        format!("{attr}_name"),
        format!("{attr}_events"),
        format!("{attr}_transitions"),
        format!("{attr}_paths"),
        format!("fire_{attr}_event"),
        format!("human_{attr}_name"),
        format!("human_{attr}_event_name"),
    ]
    .into_iter()
    .collect();
    let mut adapter_names: BTreeSet<String> = [
        format!("{attr}_event"),
        format!("{attr}_event="),
        format!("{attr}_event_transition"),
        format!("{attr}_event_transition="),
        format!("with_{attr}"),
        format!("without_{attr}"),
    ]
    .into_iter()
    .collect();
    if let Some(plural) = plural {
        adapter_names.insert(format!("with_{plural}"));
        adapter_names.insert(format!("without_{plural}"));
    }
    if option(call, "action")
        .is_some_and(|value| value.as_nil_node().is_some() || value.as_false_node().is_some())
    {
        adapter_names.remove(&format!("{attr}_event"));
        adapter_names.remove(&format!("{attr}_event="));
        adapter_names.remove(&format!("{attr}_event_transition"));
        adapter_names.remove(&format!("{attr}_event_transition="));
    }
    for state in &states {
        names.insert(match &namespace {
            Some(ns) => format!("{ns}_{state}?"),
            None => format!("{state}?"),
        });
    }
    for event in &events {
        let base = match &namespace {
            Some(ns) => format!("{event}_{ns}"),
            None => event.clone(),
        };
        names.insert(format!("{base}!"));
        names.insert(format!("can_{base}?"));
        names.insert(format!("{base}_transition"));
        names.insert(base);
    }
    vec![
        Surface {
            names,
            ancestor: None,
            provider: None,
        },
        Surface {
            names: adapter_names,
            ancestor: Some("ActiveRecord::Base"),
            provider: Some("state_machines-activerecord"),
        },
    ]
}

/// Direct machine declarations and transitions nested in event bodies,
/// not declarations inside executable callbacks or arbitrary blocks.
fn collect_machine(
    node: &Node<'_>,
    states: &mut BTreeSet<String>,
    events: &mut BTreeSet<String>,
    event_context: bool,
) -> bool {
    if let Some(call) = node.as_call_node().filter(|c| c.receiver().is_none()) {
        match call.name().as_slice() {
            b"state" if !event_context && call.block().is_none() && literal_arguments(&call) => {
                positional(&call).iter().for_each(|a| symbols(a, states))
            }
            b"event"
                if !event_context
                    && literal_arguments(&call)
                    && !positional(&call).is_empty()
                    && positional(&call).iter().all(|arg| name_of(arg).is_some()) =>
            {
                positional(&call).iter().for_each(|a| symbols(a, events))
            }
            b"transition" => {
                if !event_context {
                    let Some(on) = option(&call, "on") else {
                        return false;
                    };
                    // The gem executes this registration only for its
                    // event targets. Do not infer states from an empty
                    // set or a matcher requiring registration order.
                    if on.as_symbol_node().is_none()
                        && !on.as_array_node().is_some_and(|array| {
                            !array.elements().is_empty()
                                && array
                                    .elements()
                                    .iter()
                                    .all(|name| name.as_symbol_node().is_some())
                        })
                    {
                        return false;
                    }
                }
                let Some(pairs) = transition_pairs(&call) else {
                    return false;
                };
                for pair in pairs {
                    let key = name_of(&pair.key());
                    match key.as_deref() {
                        Some("on" | "except_on") if !event_context => {
                            symbols(&pair.value(), events)
                        }
                        Some("on" | "except_on") => return false,
                        // Method names, not states.
                        Some(
                            "if" | "unless" | "if_state" | "unless_state" | "if_all_states"
                            | "unless_all_states" | "if_any_state" | "unless_any_state",
                        ) => {}
                        Some("from" | "to" | "except_from" | "except_to") => {
                            symbols(&pair.value(), states)
                        }
                        _ => {
                            symbols(&pair.key(), states);
                            symbols(&pair.value(), states);
                        }
                    }
                }
            }
            // These registrations defer their blocks; their bodies do
            // not declare machine names at class-definition time.
            b"before_transition" | b"after_transition" | b"around_transition"
                if !event_context && literal_arguments(&call) => {}
            _ => return false,
        }
        if let Some(block) = call
            .block()
            .filter(|_| call.name().as_slice() == b"event")
            .and_then(|b| b.as_block_node())
            .and_then(|b| b.body())
        {
            if !collect_machine(&block, states, events, true) {
                return false;
            }
        }
        true
    } else if let Some(statements) = node.as_statements_node() {
        statements
            .body()
            .iter()
            .all(|statement| collect_machine(&statement, states, events, event_context))
    } else {
        false
    }
}

/// Ruby supplies the effective transition hash, not overwritten source
/// entries. Refuse opaque arguments, splats and repeated keys before
/// deriving any states. Arguments execute during registration; guard
/// lambdas defer their bodies, unlike arbitrary argument expressions.
fn transition_pairs<'pr>(call: &CallNode<'pr>) -> Option<Vec<ruby_prism::AssocNode<'pr>>> {
    let mut pairs = Vec::new();
    let mut keys = BTreeSet::new();
    let mut complex_key = false;
    for arg in call
        .arguments()
        .iter()
        .flat_map(|args| args.arguments().iter())
    {
        let elements = match (arg.as_keyword_hash_node(), arg.as_hash_node()) {
            (Some(hash), _) => hash.elements(),
            (_, Some(hash)) => hash.elements(),
            _ => return None,
        };
        for element in elements.iter() {
            let pair = element.as_assoc_node()?;
            let key = pair.key();
            if key.as_symbol_node().is_some() {
                if !keys.insert(name_of(&key)?) {
                    return None;
                }
            } else {
                // One literal array/matcher cannot overwrite a symbol
                // key. Multiple complex keys need equality semantics
                // outside this subset; strings are not keyword aliases.
                if complex_key || !literal_transition_key(&key) {
                    return None;
                }
                complex_key = true;
            }
            if !literal_transition_value(&pair.value()) {
                return None;
            }
            pairs.push(pair);
        }
    }
    Some(pairs)
}

fn literal_transition_value(node: &Node<'_>) -> bool {
    if name_of(node).is_some()
        || node.as_nil_node().is_some()
        || node.as_true_node().is_some()
        || node.as_false_node().is_some()
        || node.as_integer_node().is_some()
        || node.as_float_node().is_some()
        || node.as_lambda_node().is_some()
    {
        return true;
    }
    if let Some(array) = node.as_array_node() {
        return array
            .elements()
            .iter()
            .all(|value| literal_transition_value(&value));
    }
    if let Some(hash) = node.as_hash_node() {
        return hash.elements().iter().all(|element| {
            element.as_assoc_node().is_some_and(|pair| {
                literal_transition_value(&pair.key()) && literal_transition_value(&pair.value())
            })
        });
    }
    node.as_call_node().is_some_and(|call| {
        call.block().is_none()
            && ((call.receiver().is_none()
                && call.arguments().is_none()
                && call.name().as_slice() == b"same")
                || literal_transition_key(node))
    })
}

fn literal_transition_key(node: &Node<'_>) -> bool {
    if let Some(array) = node.as_array_node() {
        return array
            .elements()
            .iter()
            .all(|element| name_of(&element).is_some());
    }
    let Some(call) = node.as_call_node() else {
        return false;
    };
    if call.block().is_some() {
        return false;
    }
    if call.receiver().is_none() {
        return matches!(call.name().as_slice(), b"all" | b"any") && call.arguments().is_none();
    }
    call.name().as_slice() == b"-"
        && call
            .receiver()
            .is_some_and(|receiver| literal_transition_key(&receiver))
        && call.arguments().is_some_and(|args| {
            let mut args = args.arguments().iter();
            args.next()
                .is_some_and(|arg| arg.as_array_node().is_some() && literal_transition_key(&arg))
                && args.next().is_none()
        })
}

/// Symbols in a state position: a literal, an array of them, or the
/// operands of `all - [:parked]`. `all`, `any` and `same` name none.
fn symbols(node: &Node<'_>, out: &mut BTreeSet<String>) {
    if let Some(name) = node.as_symbol_node().and_then(|_| name_of(node)) {
        out.insert(name);
    } else if let Some(array) = node.as_array_node() {
        array.elements().iter().for_each(|e| symbols(&e, out));
    } else if let Some(call) = node.as_call_node() {
        if call.name().as_slice() == b"-"
            && call.receiver().is_some_and(|receiver| {
                receiver.as_call_node().is_some_and(|matcher| {
                    matcher.receiver().is_none()
                        && matches!(matcher.name().as_slice(), b"all" | b"any")
                        && matcher.arguments().is_none()
                })
            })
        {
            positional(&call).iter().for_each(|arg| symbols(arg, out));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::harvest_body;
    use super::*;

    const AASM: &str = "class A < ApplicationRecord
  aasm column: :state do
    state :draft, initial: true
    state :published
    event(:publish) { transitions from: :draft, to: :published }
  end
end";
    const AASM_NAMESPACED: &str = "class B < ApplicationRecord
  aasm(:review, column: :review, namespace: :rev) do
    state :pending, initial: true
    state :ok
    event(:approve) { transitions from: :pending, to: :ok }
  end
end";
    const MACHINE: &str = "class C < ApplicationRecord
  state_machine :status, initial: :open do
    event(:close) { transition open: :closed }
    event(:reopen) { transition all - [:open] => :open, :archived => same }
    state :limbo
  end
end";
    const MACHINE_NAMESPACED: &str = "class D < ApplicationRecord
  state_machine :alarm_state, initial: :active, namespace: \"alarm\" do
    event(:enable) { transition all => :active }
    event(:disable) { transition from: [:active], to: :off }
  end
  state_machine :state do
    event(:go) { transition :idle => :running }
  end
end";

    fn harvest(source: &str) -> Vec<(&'static str, Vec<String>)> {
        let parsed = ruby_prism::parse(source.as_bytes());
        let program = parsed.node();
        let class = program
            .as_program_node()
            .unwrap()
            .statements()
            .body()
            .iter()
            .next()
            .unwrap();
        let body = class
            .as_class_node()
            .and_then(|c| c.body())
            .or_else(|| class.as_module_node().and_then(|m| m.body()))
            .unwrap();
        let mut out = Vec::new();
        harvest_body(&body, &mut out);
        out.into_iter()
            .map(|g| {
                (
                    g.dsl,
                    g.surfaces
                        .into_iter()
                        .flat_map(|surface| surface.names)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                )
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    // Pinned expectations have a reproducible external oracle: the
    // ignored test below diffs the real gems' methods using these same
    // source fixtures. Default tests require no installed native gems.

    #[test]
    fn aasm_states_and_events() {
        let got = harvest(AASM);
        assert_eq!(
            got,
            vec![(
                "aasm",
                names(&[
                    "draft?",
                    "published?",
                    "draft",
                    "published",
                    "publish",
                    "publish!",
                    "publish_without_validation!",
                    "may_publish?",
                ])
            )]
        );
    }

    #[test]
    fn aasm_namespace() {
        let got = harvest(AASM_NAMESPACED);
        assert_eq!(
            got,
            vec![(
                "aasm",
                names(&[
                    "rev_pending?",
                    "rev_ok?",
                    "pending",
                    "ok",
                    "rev_pending",
                    "rev_ok",
                    "approve_rev",
                    "approve_rev!",
                    "approve_rev_without_validation!",
                    "may_approve_rev?",
                ])
            )]
        );
    }

    #[test]
    fn state_machine_states_from_transitions() {
        let got = harvest(MACHINE);
        assert_eq!(
            got,
            vec![(
                "state_machine",
                names(&[
                    "archived?",
                    "closed?",
                    "limbo?",
                    "open?",
                    "close",
                    "close!",
                    "can_close?",
                    "close_transition",
                    "reopen",
                    "reopen!",
                    "can_reopen?",
                    "reopen_transition",
                    "status?",
                    "status_event",
                    "status_event=",
                    "status_event_transition",
                    "status_event_transition=",
                    "status_events",
                    "status_name",
                    "status_paths",
                    "status_transitions",
                    "fire_status_event",
                    "human_status_name",
                    "human_status_event_name",
                    "with_status",
                    "with_statuses",
                    "without_status",
                    "without_statuses",
                ])
            )]
        );
    }

    #[test]
    fn state_machine_namespace_and_default_attribute() {
        let got = harvest(MACHINE_NAMESPACED);
        assert_eq!(
            got,
            vec![
                (
                    "state_machine",
                    names(&[
                        "alarm_active?",
                        "alarm_off?",
                        "enable_alarm",
                        "enable_alarm!",
                        "can_enable_alarm?",
                        "enable_alarm_transition",
                        "disable_alarm",
                        "disable_alarm!",
                        "can_disable_alarm?",
                        "disable_alarm_transition",
                        "alarm_state?",
                        "alarm_state_event",
                        "alarm_state_event=",
                        "alarm_state_event_transition",
                        "alarm_state_event_transition=",
                        "alarm_state_events",
                        "alarm_state_name",
                        "alarm_state_paths",
                        "alarm_state_transitions",
                        "fire_alarm_state_event",
                        "human_alarm_state_name",
                        "human_alarm_state_event_name",
                        "with_alarm_state",
                        "with_alarm_states",
                        "without_alarm_state",
                        "without_alarm_states",
                    ])
                ),
                (
                    "state_machine",
                    names(&[
                        "idle?",
                        "running?",
                        "go",
                        "go!",
                        "can_go?",
                        "go_transition",
                        "state?",
                        "state_event",
                        "state_event=",
                        "state_event_transition",
                        "state_event_transition=",
                        "state_events",
                        "state_name",
                        "state_paths",
                        "state_transitions",
                        "fire_state_event",
                        "human_state_name",
                        "human_state_event_name",
                        "with_state",
                        "with_states",
                        "without_state",
                        "without_states",
                    ])
                ),
            ]
        );
    }

    #[test]
    fn concern_blocks_and_non_declarations() {
        let got = harvest(
            "module Workflow
  extend ActiveSupport::Concern
  included do
    with_options do
      state_machine :delivery_status do
        event(:deliver) { transition waiting: :delivered, if: :ready? }
      end
    end
  end
  aasm
  self.aasm do; end
  def ordinary
    aasm do; end
  end
end",
        );
        assert_eq!(
            got.len(),
            3,
            "blockless/explicit-self calls remain vetoes; method bodies do not"
        );
        assert!(got[1..].iter().all(|(_, names)| names.is_empty()));
        let (dsl, found) = &got[0];
        assert_eq!(*dsl, "state_machine");
        assert!(found.contains(&"deliver!".to_string()));
        assert!(found.contains(&"delivered?".to_string()));
        assert!(!found.contains(&"ready??".to_string()) && !found.contains(&"ready?".to_string()));
    }

    #[test]
    #[ignore = "requires pinned native gems listed in tests/gem_generated_surface_probe.rb"]
    fn native_generator_rules_match_pinned_gems() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut cases = vec![
            ("A", AASM.to_string()),
            ("B", AASM_NAMESPACED.to_string()),
            ("C", MACHINE.to_string()),
            ("D", MACHINE_NAMESPACED.to_string()),
            (
                "Native",
                "class Native < ApplicationRecord\n  aasm do; state :draft, :published; end\nend"
                    .to_string(),
            ),
        ];
        for options in ["plural: :workflow_statuses", "action: nil", "action: false"] {
            cases.push(("Native", format!("class Native < ApplicationRecord\n  state_machine :status, {options} do; event :finish; end\nend")));
        }
        for guard in [
            "if_state",
            "unless_state",
            "if_all_states",
            "unless_all_states",
            "if_any_state",
            "unless_any_state",
        ] {
            cases.push(("Native", format!("class Native < ApplicationRecord\n  state_machine :status do; event(:finish) {{ transition open: :closed, {guard}: {{status: :open}} }}; end\nend")));
        }
        for (owner, source) in cases {
            // Every fixture runs in a fresh process, avoiding accidental
            // reopens between cases. Expectations come from the gems,
            // not a second implementation of the naming algorithm.
            let mut child = Command::new("ruby")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/gem_generated_surface_probe.rb"
                ))
                .arg(owner)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("ruby on PATH with the pinned gems installed");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(source.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{source}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let native: BTreeSet<String> = serde_json::from_slice(&output.stdout).unwrap();
            let generated: BTreeSet<_> = harvest(&source)
                .into_iter()
                .flat_map(|(_, names)| names)
                .collect();
            assert_eq!(generated, native, "native surface differs for {source}");
        }
    }
}
